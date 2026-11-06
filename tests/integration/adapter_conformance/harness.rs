//! Harness-side supervisor test double for the adapter conformance suite.
//!
//! Speaks the frozen adapter wire (newline-delimited JSON-RPC 2.0 over stdio)
//! from the supervisor side: frame read/write, hello-request answering,
//! transcript capture, and transport teardown. The double implements no
//! supervision policy beyond what a test drives explicitly — bounded-trigger
//! resolution and admission decisions live in the tests, so the double stays
//! a faithful pipe with a captured transcript.

use std::{
    io::{BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};

/// One captured wire line with its direction.
pub(crate) struct TranscriptLine {
    pub(crate) direction: Direction,
    pub(crate) raw: String,
}

/// Which side of the pipe emitted a transcript line.
pub(crate) enum Direction {
    ToAdapter,
    FromAdapter,
}

/// Outcome of a bounded frame read.
pub(crate) enum ReadOutcome {
    Frame(Value),
    Timeout,
    Unparseable(String),
    Incomplete(String),
    Closed,
    ReadFailed(String),
}

/// One item from the stdout forwarder: a read line with its delimiter
/// status, a true empty EOF, or an observed read failure. Delimiter and
/// read-error status travel with the bytes so a newline-less final
/// frame can never pass as a complete one.
pub(crate) enum PipeRead {
    Line { text: String, terminated: bool },
    Eof,
    Failed(String),
}

/// Maps one forwarder item to its read outcome. Pure to keep the
/// delimiter and read-error seam unit-testable without real I/O
/// failures.
fn classify(read: PipeRead) -> ReadOutcome {
    match read {
        PipeRead::Line { text, terminated } => {
            let line = text.trim_end().to_string();
            if !terminated {
                return ReadOutcome::Incomplete(line);
            }
            match serde_json::from_str(&line) {
                Ok(frame) => ReadOutcome::Frame(frame),
                Err(_) => ReadOutcome::Unparseable(line),
            }
        },
        PipeRead::Eof => ReadOutcome::Closed,
        PipeRead::Failed(reason) => ReadOutcome::ReadFailed(reason),
    }
}

/// Originator-monotonic wire ids, decimal strings from 1, never reused.
pub(crate) struct WireIds {
    next: u64,
}

impl WireIds {
    fn fresh() -> Self {
        Self { next: 1 }
    }

    pub(crate) fn mint(&mut self) -> String {
        let id = self.next;
        self.next += 1;
        id.to_string()
    }
}

/// An adapter-originated `handshakeHello` call awaiting its result.
pub(crate) struct HelloCall {
    pub(crate) wire_id: String,
    pub(crate) params: Value,
}

/// Supervisor side of one adapter process pipe.
///
/// Dropping the double reaps the child within a bound: every early
/// return and every panic unwinds through `Drop`, so a defective
/// adapter can never leave a live child behind the test.
pub(crate) struct SupervisorDouble {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<PipeRead>,
    transcript: Vec<TranscriptLine>,
    ids: WireIds,
}

/// Strictly decodes one wire line: invalid UTF-8 is a failure with a
/// byte-exact detail, never silently replaced with U+FFFD. Pure for
/// testability; the forwarder applies it to every emitted line.
fn decode_line(raw: Vec<u8>) -> Result<String, String> {
    String::from_utf8(raw)
        .map_err(|source| format!("non-UTF8 line: {:?}", source.into_bytes()))
}

fn spawn_line_forwarder(stdout: ChildStdout) -> mpsc::Receiver<PipeRead> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut pending: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) => {
                    if !pending.is_empty() {
                        let raw: Vec<u8> = std::mem::take(&mut pending);
                        let terminated = false;
                        match decode_line(raw) {
                            Ok(text) => {
                                let _ = sender
                                    .send(PipeRead::Line { text, terminated });
                            },
                            Err(reason) => {
                                let _ = sender.send(PipeRead::Failed(reason));
                            },
                        }
                    }
                    let _ = sender.send(PipeRead::Eof);
                    return;
                },
                Err(source) => {
                    let _ = sender.send(PipeRead::Failed(source.to_string()));
                    return;
                },
                Ok(count) => {
                    pending.extend_from_slice(&chunk[..count]);
                    while let Some(end) =
                        pending.iter().position(|byte| *byte == b'\n')
                    {
                        let raw: Vec<u8> = pending.drain(..=end).collect();
                        match decode_line(raw) {
                            Ok(text) => {
                                let terminated = true;
                                if sender
                                    .send(PipeRead::Line { text, terminated })
                                    .is_err()
                                {
                                    return;
                                }
                            },
                            Err(reason) => {
                                let _ = sender.send(PipeRead::Failed(reason));
                            },
                        }
                    }
                },
            }
        }
    });
    receiver
}

impl SupervisorDouble {
    /// Spawns `program` with piped stdio. The child speaks the adapter wire.
    /// Single-process scope: teardown signals the direct child only, which
    /// suffices for the scripted generic profile (no agent child). Stub
    /// scripts in the self-tests use `exec` so no grandchild outlives the
    /// direct child; multi-process teardown belongs to the real
    /// supervisor, not to this pipe.
    pub(crate) fn spawn(
        program: &str,
        arguments: &[&str],
    ) -> std::io::Result<Self> {
        Self::spawn_with_env(program, arguments, &[])
    }

    /// Spawns `program` with piped stdio and extra environment variables.
    pub(crate) fn spawn_with_env(
        program: &str,
        arguments: &[&str],
        environment: &[(&str, &str)],
    ) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(arguments)
            .envs(environment.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped adapter stdin");
        let stdout = child.stdout.take().expect("piped adapter stdout");
        Ok(Self {
            child,
            stdin: Some(stdin),
            lines: spawn_line_forwarder(stdout),
            transcript: Vec::new(),
            ids: WireIds::fresh(),
        })
    }

    fn stdin(&mut self) -> &mut ChildStdin {
        self.stdin.as_mut().expect("adapter stdin still open")
    }

    /// Sends one frame and records it on the transcript.
    pub(crate) fn send_frame(&mut self, frame: &Value) {
        let raw = serde_json::to_string(frame).expect("encode frame");
        self.send_raw(&raw);
    }

    /// Sends one raw line and records it on the transcript. Malformed
    /// input goes through here; well-formed frames use `send_frame`.
    pub(crate) fn send_raw(&mut self, raw: &str) {
        writeln!(self.stdin(), "{raw}").expect("write frame");
        self.stdin().flush().expect("flush frame");
        self.transcript.push(TranscriptLine {
            direction: Direction::ToAdapter,
            raw: raw.to_string(),
        });
    }

    /// Closes the write end, delivering EOF without tearing the pipe down.
    /// Reads continue until the child exits or the test disconnects.
    pub(crate) fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Reads one frame within `deadline`, recording it on the transcript.
    /// A closed pipe reads as `Closed`, never as a timeout, so tests can
    /// distinguish a quiet child from a dead one. Lines that arrive
    /// without their newline read as `Incomplete`, never as frames, so a
    /// truncated final line cannot pass as a complete one.
    pub(crate) fn read_frame_deadline(
        &mut self,
        deadline: Duration,
    ) -> ReadOutcome {
        match self.lines.recv_timeout(deadline) {
            Ok(PipeRead::Line { text, terminated }) => {
                let line = text.trim_end().to_string();
                self.transcript.push(TranscriptLine {
                    direction: Direction::FromAdapter,
                    raw: line.clone(),
                });
                classify(PipeRead::Line {
                    text: line,
                    terminated,
                })
            },
            Ok(PipeRead::Eof) => ReadOutcome::Closed,
            Ok(PipeRead::Failed(reason)) => ReadOutcome::ReadFailed(reason),
            Err(mpsc::RecvTimeoutError::Timeout) => ReadOutcome::Timeout,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                ReadOutcome::ReadFailed(
                    "forwarder ended without an EOF marker".to_string(),
                )
            },
        }
    }

    /// Sends a supervisor-originated request, returning its wire id.
    pub(crate) fn request(&mut self, method: &str, params: Value) -> String {
        let id = self.ids.mint();
        self.send_frame(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }));
        id
    }

    /// Sends a supervisor-originated notification (no wire id).
    pub(crate) fn notify(&mut self, method: &str, params: Value) {
        self.send_frame(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }));
    }

    /// Reads the adapter-originated `handshakeHello` call within `deadline`.
    /// Only a call carrying the version offer (`protocol_major` plus
    /// `protocol_minor` on params, per the contract) is admitted; anything
    /// else reads as absent and stays on the transcript for the test to
    /// inspect.
    pub(crate) fn expect_hello(
        &mut self,
        deadline: Duration,
    ) -> Option<HelloCall> {
        match self.read_frame_deadline(deadline) {
            ReadOutcome::Frame(frame) => {
                if frame["method"] != "adapter.handshakeHello" {
                    return None;
                }
                let params = frame["params"].clone();
                if params["protocol_major"].is_number()
                    && params["protocol_minor"].is_number()
                {
                    Some(HelloCall {
                        wire_id: frame["id"]
                            .as_str()
                            .unwrap_or_default()
                            .to_string(),
                        params,
                    })
                } else {
                    None
                }
            },
            _ => None,
        }
    }

    /// Answers a hello call with the supervisor-issued binding triple.
    pub(crate) fn answer_hello(&mut self, call: &HelloCall, result: Value) {
        self.send_frame(&json!({
            "jsonrpc": "2.0",
            "id": call.wire_id,
            "result": result,
        }));
    }

    /// Answers an open adapter-originated call with a result payload.
    pub(crate) fn respond(&mut self, wire_id: &str, result: Value) {
        self.send_frame(&json!({
            "jsonrpc": "2.0",
            "id": wire_id,
            "result": result,
        }));
    }

    /// Answers an open adapter-originated call with an error payload.
    pub(crate) fn fail(&mut self, wire_id: &str, error: Value) {
        self.send_frame(&json!({
            "jsonrpc": "2.0",
            "id": wire_id,
            "error": error,
        }));
    }

    /// Asserts no frame arrives within `duration`. A closed pipe fails
    /// the assertion: silence means a live but quiet child, not a dead
    /// one. A truncated line or a read failure fails it too — quiet
    /// never hides a damaged pipe.
    pub(crate) fn expect_quiet(
        &mut self,
        duration: Duration,
    ) -> Result<(), String> {
        match self.read_frame_deadline(duration) {
            ReadOutcome::Timeout => Ok(()),
            ReadOutcome::Frame(frame) => {
                Err(format!("expected quiet, got frame {frame}"))
            },
            ReadOutcome::Unparseable(raw) => {
                Err(format!("expected quiet, got unparseable {raw:?}"))
            },
            ReadOutcome::Incomplete(raw) => {
                Err(format!("expected quiet, got truncated {raw:?}"))
            },
            ReadOutcome::ReadFailed(reason) => {
                Err(format!("expected quiet, read failed: {reason}"))
            },
            ReadOutcome::Closed => {
                Err("expected quiet, pipe closed".to_string())
            },
        }
    }

    /// Reports whether the pipe closes within `deadline`.
    pub(crate) fn expect_eof(&mut self, deadline: Duration) -> bool {
        matches!(self.read_frame_deadline(deadline), ReadOutcome::Closed)
    }

    /// A well-formed hello result assigning a fresh binding triple.
    pub(crate) fn hello_result(
        runtime_instance: &str,
        adapter_generation: u64,
    ) -> Value {
        json!({
            "runtime_instance": runtime_instance,
            "adapter_generation": adapter_generation,
            "selected_major": 1,
            "selected_minor": 0,
            "negotiated_limits": {
                "max_frame_bytes": 1048576,
                "queue_tx_bytes": 16777216,
                "queue_rx_bytes": 16777216,
                "max_control_pending": 32,
                "max_events_per_second": 100,
                "window_seconds": 1,
                "max_submits_in_flight": 64,
                "max_choices_in_flight": 16,
                "max_looks_in_flight": 8,
                "drain_wait_ms": 250,
            },
            "snapshot_b64": "",
        })
    }

    /// Closes stdin, reaps the child within a bound, and returns the
    /// transcript. Escalates to a kill when the child ignores EOF.
    pub(crate) fn disconnect(mut self) -> Vec<TranscriptLine> {
        self.teardown(Duration::from_secs(5));
        std::mem::take(&mut self.transcript)
    }

    #[cfg(test)]
    fn disconnect_with_grace(
        mut self,
        grace: Duration,
    ) -> Vec<TranscriptLine> {
        self.teardown(grace);
        std::mem::take(&mut self.transcript)
    }

    /// Closes the write end, kills if still alive past `grace`, and
    /// reaps within a further bound. Teardown is polled explicitly
    /// with bounds throughout: it never blocks on a child that
    /// ignores EOF, and never trusts an un-reaped wait.
    fn teardown(&mut self, grace: Duration) {
        self.stdin = None;
        let deadline = Instant::now() + grace;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                },
                Err(_) => break,
            }
        }
        let _ = self.child.kill();
        self.reap();
    }

    /// Reaps the child within a fixed bound, ignoring the outcome.
    /// Safe to call on an already-reaped child.
    fn reap(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                },
            }
        }
    }
}

impl Drop for SupervisorDouble {
    /// Reaps the child on every path, including early returns and
    /// panics: the runner's mismatch exits all drop the double, so a
    /// defective adapter can never leave a live child behind the
    /// test. Bounded and infallible by construction.
    fn drop(&mut self) {
        self.stdin = None;
        let _ = self.child.kill();
        self.reap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_ids_mint_monotonic_decimals_from_one() {
        let mut ids = WireIds::fresh();
        assert_eq!(ids.mint(), "1");
        assert_eq!(ids.mint(), "2");
        assert_eq!(ids.mint(), "3");
    }

    #[test]
    fn hello_exchange_captures_both_transcript_directions() {
        let mut double = SupervisorDouble::spawn(
            "sh",
            &[
                "-c",
                r#"printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"adapter.handshakeHello","params":{"adapter_profile":"generic","protocol_major":1,"protocol_minor":0}}'; read -r line"#,
            ],
        )
        .expect("spawn stub adapter");
        let call = double
            .expect_hello(Duration::from_secs(5))
            .expect("stub emits hello");
        assert_eq!(call.wire_id, "1");
        assert_eq!(call.params["adapter_profile"], "generic");
        double.answer_hello(
            &call,
            SupervisorDouble::hello_result("instance-1", 7),
        );
        let transcript = double.disconnect();
        assert_eq!(transcript.len(), 2);
        assert!(matches!(transcript[0].direction, Direction::FromAdapter));
        assert!(matches!(transcript[1].direction, Direction::ToAdapter));
        assert!(transcript[1].raw.contains("instance-1"));
    }

    #[test]
    fn hello_without_version_offer_reads_as_absent() {
        let mut double = SupervisorDouble::spawn(
            "sh",
            &[
                "-c",
                r#"printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"adapter.handshakeHello","params":{"adapter_profile":"generic"}}'; read -r line"#,
            ],
        )
        .expect("spawn version-less stub");
        let call = double.expect_hello(Duration::from_secs(5));
        assert!(call.is_none());
        let transcript = double.disconnect();
        assert_eq!(transcript.len(), 1);
        assert!(transcript[0].raw.contains("adapter.handshakeHello"));
    }

    #[test]
    fn quiet_child_reads_timeout_within_deadline() {
        let mut double =
            SupervisorDouble::spawn("sh", &["-c", "read -r line || exit 0"])
                .expect("spawn quiet stub");
        let started = Instant::now();
        let outcome = double.read_frame_deadline(Duration::from_millis(300));
        assert!(matches!(outcome, ReadOutcome::Timeout));
        assert!(started.elapsed() < Duration::from_secs(5));
        let transcript = double.disconnect();
        assert!(transcript.is_empty());
    }

    #[test]
    fn request_mints_wire_id_and_reads_matching_response() {
        let mut double = SupervisorDouble::spawn(
            "sh",
            &[
                "-c",
                r#"read -r line; printf '%s\n' '{"jsonrpc":"2.0","id":"1","result":{"ok":true}}'"#,
            ],
        )
        .expect("spawn echo stub");
        let id = double.request("relay.ping", json!({}));
        assert_eq!(id, "1");
        match double.read_frame_deadline(Duration::from_secs(5)) {
            ReadOutcome::Frame(frame) => assert_eq!(frame["id"], "1"),
            _ => panic!("expected the stub response frame"),
        }
        double.disconnect();
    }

    #[test]
    fn notify_records_unsolicited_frame_on_transcript() {
        let mut double =
            SupervisorDouble::spawn("sh", &["-c", "read -r line"])
                .expect("spawn reader stub");
        double.notify(
            "relay.supervisionSignal",
            json!({"signal": "fence-hint"}),
        );
        let transcript = double.disconnect();
        assert_eq!(transcript.len(), 1);
        assert!(matches!(transcript[0].direction, Direction::ToAdapter));
        assert!(transcript[0].raw.contains("fence-hint"));
    }

    #[test]
    fn garbage_line_reports_unparseable_payload() {
        let mut double = SupervisorDouble::spawn(
            "sh",
            &["-c", r#"printf '%s\n' 'not-json'; read -r line || exit 0"#],
        )
        .expect("spawn garbage stub");
        match double.read_frame_deadline(Duration::from_secs(5)) {
            ReadOutcome::Unparseable(raw) => assert_eq!(raw, "not-json"),
            _ => panic!("expected the unparseable outcome"),
        }
        double.disconnect();
    }

    #[test]
    fn respond_answers_an_open_call_by_wire_id() {
        let mut double =
            SupervisorDouble::spawn("sh", &["-c", "read -r line"])
                .expect("spawn reader stub");
        double
            .respond("4", json!({"outcome": {"chosen": {"option_id": "a"}}}));
        let transcript = double.disconnect();
        assert_eq!(transcript.len(), 1);
        assert!(transcript[0].raw.contains("\"outcome\""));
    }

    #[test]
    fn fail_sends_error_frame_for_captured_id() {
        let mut double =
            SupervisorDouble::spawn("sh", &["-c", "read -r line"])
                .expect("spawn reader stub");
        double
            .fail("9", json!({"code": -32600, "message": "Invalid Request"}));
        let transcript = double.disconnect();
        assert_eq!(transcript.len(), 1);
        assert!(transcript[0].raw.contains("\"error\""));
        assert!(transcript[0].raw.contains("\"id\":\"9\""));
    }

    #[test]
    fn kill_escalation_reaps_a_child_that_ignores_eof() {
        let double = SupervisorDouble::spawn("sh", &["-c", "exec sleep 30"])
            .expect("spawn sleeper stub");
        let started = Instant::now();
        let transcript =
            double.disconnect_with_grace(Duration::from_millis(100));
        assert!(transcript.is_empty());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn classify_marks_delimiter_and_read_status() {
        match classify(PipeRead::Line {
            text: "{\"a\":1}\n".to_string(),
            terminated: true,
        }) {
            ReadOutcome::Frame(frame) => assert_eq!(frame["a"], 1),
            _ => panic!("terminated JSON is a frame"),
        }
        match classify(PipeRead::Line {
            text: "nope\n".to_string(),
            terminated: true,
        }) {
            ReadOutcome::Unparseable(raw) => assert_eq!(raw, "nope"),
            _ => panic!("terminated garbage is unparseable"),
        }
        match classify(PipeRead::Line {
            text: "{\"a\":1}".to_string(),
            terminated: false,
        }) {
            ReadOutcome::Incomplete(raw) => assert_eq!(raw, "{\"a\":1}"),
            _ => panic!("unterminated JSON is incomplete"),
        }
        assert!(matches!(classify(PipeRead::Eof), ReadOutcome::Closed));
        match classify(PipeRead::Failed("boom".to_string())) {
            ReadOutcome::ReadFailed(reason) => assert_eq!(reason, "boom"),
            _ => panic!("read failure stays a failure"),
        }
    }

    #[test]
    fn decode_line_rejects_invalid_utf8_without_replacement() {
        assert_eq!(
            decode_line(b"{\"a\":1}\n".to_vec()),
            Ok("{\"a\":1}\n".to_string())
        );
        let detail = decode_line(vec![0xff, 0xfe, b'\n'])
            .expect_err("invalid UTF-8 must fail");
        assert!(detail.contains("[255, 254, 10]"), "detail was {detail}");
    }

    #[test]
    fn non_utf8_line_reads_as_read_failed() {
        let mut double = SupervisorDouble::spawn(
            "sh",
            &["-c", r#"printf '\377\376\n'; exec sleep 30"#],
        )
        .expect("spawn non-UTF8 stub");
        match double.read_frame_deadline(Duration::from_secs(5)) {
            ReadOutcome::ReadFailed(reason) => {
                assert!(reason.contains("non-UTF8"), "{reason}")
            },
            _ => panic!("expected the read-failed outcome"),
        }
        double.disconnect();
    }

    #[test]
    fn unterminated_valid_json_reads_as_incomplete() {
        let mut double =
            SupervisorDouble::spawn("sh", &["-c", r#"printf '%s' '{"a":1}'"#])
                .expect("spawn delimiter-less stub");
        match double.read_frame_deadline(Duration::from_secs(5)) {
            ReadOutcome::Incomplete(raw) => assert_eq!(raw, "{\"a\":1}"),
            _ => panic!("expected the incomplete outcome"),
        }
        double.disconnect();
    }

    #[test]
    fn dropped_double_reaps_a_live_child_on_mismatch() {
        let directory = tempfile::tempdir().expect("tempdir");
        let pid_file = directory.path().join("pid");
        let script = format!(
            "echo $$ > {}; printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":\"1\",\"method\":\"adapter.handshakeHello\",\"params\":{{\"adapter_profile\":\"generic\",\"protocol_major\":1,\"protocol_minor\":0}}}}'; printf '%s\\n' 'bogus'; exec sleep 30",
            pid_file.display()
        );
        let mut double = SupervisorDouble::spawn("sh", &["-c", &script])
            .expect("spawn sleeper stub");
        let call = double
            .expect_hello(Duration::from_secs(5))
            .expect("stub emits hello");
        double.answer_hello(
            &call,
            SupervisorDouble::hello_result("instance-1", 7),
        );
        match double.read_frame_deadline(Duration::from_secs(5)) {
            ReadOutcome::Unparseable(_) => {},
            _ => panic!("expected the stub garbage"),
        }
        drop(double);
        let end = Instant::now() + Duration::from_secs(5);
        let pid: i32 = loop {
            if let Ok(text) = std::fs::read_to_string(&pid_file)
                && let Ok(pid) = text.trim().parse()
            {
                break pid;
            }
            if Instant::now() >= end {
                panic!("stub never wrote its pid");
            }
            thread::sleep(Duration::from_millis(20));
        };
        loop {
            let gone = unsafe {
                libc::kill(pid, 0) != 0
                    && std::io::Error::last_os_error().raw_os_error()
                        == Some(libc::ESRCH)
            };
            if gone || Instant::now() >= end {
                assert!(gone, "sleeper child still alive after drop");
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
