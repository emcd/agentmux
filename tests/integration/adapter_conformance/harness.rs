//! Harness-side supervisor test double for the adapter conformance suite.
//!
//! Speaks the frozen adapter wire (newline-delimited JSON-RPC 2.0 over stdio)
//! from the supervisor side: frame read/write, hello-request answering,
//! transcript capture, and transport teardown. The double implements no
//! supervision policy beyond what a test drives explicitly — bounded-trigger
//! resolution and admission decisions live in the tests, so the double stays
//! a faithful pipe with a captured transcript.

use std::{
    io::{BufRead, BufReader, Write},
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
pub(crate) struct SupervisorDouble {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    transcript: Vec<TranscriptLine>,
    ids: WireIds,
}

fn spawn_line_forwarder(stdout: ChildStdout) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if sender.send(line).is_err() {
                        return;
                    }
                },
            }
        }
    });
    receiver
}

impl SupervisorDouble {
    /// Spawns `program` with piped stdio. The child speaks the adapter wire.
    pub(crate) fn spawn(
        program: &str,
        arguments: &[&str],
    ) -> std::io::Result<Self> {
        let mut child = Command::new(program)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().expect("piped adapter stdin");
        let stdout = child.stdout.take().expect("piped adapter stdout");
        Ok(Self {
            child,
            stdin,
            lines: spawn_line_forwarder(stdout),
            transcript: Vec::new(),
            ids: WireIds::fresh(),
        })
    }

    /// Sends one frame and records it on the transcript.
    pub(crate) fn send_frame(&mut self, frame: &Value) {
        let raw = serde_json::to_string(frame).expect("encode frame");
        writeln!(self.stdin, "{raw}").expect("write frame");
        self.stdin.flush().expect("flush frame");
        self.transcript.push(TranscriptLine {
            direction: Direction::ToAdapter,
            raw,
        });
    }

    /// Reads one frame within `deadline`, recording it on the transcript.
    pub(crate) fn read_frame_deadline(
        &mut self,
        deadline: Duration,
    ) -> ReadOutcome {
        let raw = match self.lines.recv_timeout(deadline) {
            Ok(line) => line,
            Err(_) => return ReadOutcome::Timeout,
        };
        let line = raw.trim_end().to_string();
        let outcome = match serde_json::from_str(&line) {
            Ok(frame) => ReadOutcome::Frame(frame),
            Err(_) => ReadOutcome::Unparseable(line.clone()),
        };
        self.transcript.push(TranscriptLine {
            direction: Direction::FromAdapter,
            raw: line,
        });
        outcome
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
    pub(crate) fn expect_hello(
        &mut self,
        deadline: Duration,
    ) -> Option<HelloCall> {
        match self.read_frame_deadline(deadline) {
            ReadOutcome::Frame(frame) => {
                if frame["method"] != "adapter.handshakeHello" {
                    return None;
                }
                Some(HelloCall {
                    wire_id: frame["id"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                    params: frame["params"].clone(),
                })
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
    pub(crate) fn disconnect(self) -> Vec<TranscriptLine> {
        self.disconnect_with_grace(Duration::from_secs(5))
    }

    fn disconnect_with_grace(
        mut self,
        grace: Duration,
    ) -> Vec<TranscriptLine> {
        drop(self.stdin);
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
        let _ = self.child.wait();
        self.transcript
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
                r#"printf '%s\n' '{"jsonrpc":"2.0","id":"1","method":"adapter.handshakeHello","params":{"adapter_profile":"generic"}}'; read -r line"#,
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
        double.respond("4", json!({"chosen": {"option_id": "a"}}));
        let transcript = double.disconnect();
        assert_eq!(transcript.len(), 1);
        assert!(transcript[0].raw.contains("\"id\":\"4\""));
    }

    #[test]
    fn kill_escalation_reaps_a_child_that_ignores_eof() {
        let double = SupervisorDouble::spawn("sh", &["-c", "sleep 30"])
            .expect("spawn sleeper stub");
        let started = Instant::now();
        let transcript =
            double.disconnect_with_grace(Duration::from_millis(100));
        assert!(transcript.is_empty());
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
