use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::Command,
};

use agentmux::runtime::sockets::bind_unix_listener;

use crate::support::process::strip_bring_up_context;

pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const BUNDLE_NAME: &str = "party";
pub(crate) const SENDER_SESSION: &str = "alpha";

pub(crate) type RelayResponder = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// Usable `sockaddr_un.sun_path` bytes on Darwin, the tightest target this
/// suite builds for and the one where the runtime hands the full path to
/// `bind`. Mirrors the non-Linux arm of
/// [`agentmux::runtime::sockets::UNIX_SOCKET_PATH_MAXIMUM`], which resolves per
/// target and so reports the roomier Linux figure when compiled here.
const DARWIN_SOCKET_PATH_MAXIMUM: usize = 103;

/// Bytes of that limit reserved for the host's temporary directory, trailing
/// separator included. `/tmp/` spends 5; the `/var/folders/<..>/<..>/T/` path
/// macOS hands each user runs to roughly 49, which is the figure worth
/// budgeting against.
const HOST_TEMPORARY_ROOT_RESERVE: usize = 49;

pub(crate) struct TestRuntime {
    pub root: PathBuf,
    pub config_root: PathBuf,
    pub state_root: PathBuf,
    pub relay_socket: PathBuf,
    // Declared last so it is dropped last: dropping it removes `root`, and the
    // paths above must stay valid for the whole fixture lifetime.
    _temporary: TempDir,
}

impl TestRuntime {
    pub(crate) fn create() -> Self {
        // Beneath the system temporary directory rather than
        // `.auxiliary/temporary`: the relay socket under this root must fit
        // `sun_path`, and a repository-rooted fixture spends that budget on the
        // checkout path before the fixture spends any of it. The name is kept
        // short for the same reason — a `<prefix>-<pid>-<nanos>` name overflows
        // Darwin's limit even from a short root.
        //
        // The root is absolute either way, which matters independently: the
        // runtime normalizes the state root it is given and reports resolved
        // paths back, so a relative fixture root would leave assertions naming
        // a path the runtime never produces.
        let temporary = TempDir::with_prefix("mcp-").expect("create temporary root");
        let root = temporary.path().to_path_buf();
        let config_root = root.join("config");
        let state_root = root.join("state");
        let relay_socket = state_root.join("relay.sock");
        // What the fixture itself spends, rather than the assembled path: a
        // Linux run assembles that path under `/tmp` and it fits whatever the
        // fixture is named, so measuring it here would pass for a name that
        // strands macOS CI. The fixture's own share is the same on every host.
        let fixture_share = root
            .file_name()
            .expect("temporary root directory name")
            .len()
            + "/state/relay.sock".len();
        let budget = DARWIN_SOCKET_PATH_MAXIMUM - HOST_TEMPORARY_ROOT_RESERVE;
        assert!(
            fixture_share <= budget,
            "fixture spends {fixture_share} bytes of the socket path, over its \
             {budget}-byte share of Darwin's {DARWIN_SOCKET_PATH_MAXIMUM}-byte sun_path limit: {}",
            relay_socket.display()
        );

        fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
        fs::create_dir_all(&state_root).expect("create state root");
        write_bundle_configuration(
            &config_root,
            BUNDLE_NAME,
            &[SENDER_SESSION, "bravo", "charlie"],
        );

        Self {
            root,
            config_root,
            state_root,
            relay_socket,
            _temporary: temporary,
        }
    }
}

pub(crate) struct FakeRelay {
    socket_path: PathBuf,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Value>>>,
    envelopes: Arc<Mutex<Vec<Value>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FakeRelay {
    pub(crate) fn start(socket_path: PathBuf, responder: RelayResponder) -> Self {
        let mut routes: HashMap<String, RelayResponder> = HashMap::new();
        routes.insert(BUNDLE_NAME.to_string(), responder);
        routes.insert("GLOBAL".to_string(), default_empty_global_responder());
        Self::start_for_bundles(socket_path, routes)
    }

    pub(crate) fn start_for_bundles(
        socket_path: PathBuf,
        routes: HashMap<String, RelayResponder>,
    ) -> Self {
        if socket_path.exists() {
            fs::remove_file(&socket_path).expect("remove stale relay socket");
        }
        if let Some(parent) = socket_path.parent() {
            fs::create_dir_all(parent).expect("create relay socket parent");
        }
        // Through the crate's own helper rather than `UnixListener::bind`, so
        // the fixture addresses its socket exactly as the relay addresses its
        // own — including the parent-directory form Linux uses to stay
        // independent of how deep the state root sits.
        let listener = bind_unix_listener(&socket_path).expect("bind fake relay");
        listener
            .set_nonblocking(true)
            .expect("set fake relay listener nonblocking");

        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let envelopes = Arc::new(Mutex::new(Vec::new()));
        let stop_inner = Arc::clone(&stop);
        let requests_inner = Arc::clone(&requests);
        let envelopes_inner = Arc::clone(&envelopes);
        let socket_path_inner = socket_path.clone();
        let routes = Arc::new(routes);

        let thread = thread::spawn(move || {
            while !stop_inner.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _address)) => {
                        let routes = Arc::clone(&routes);
                        let requests_for_conn = Arc::clone(&requests_inner);
                        let envelopes_for_conn = Arc::clone(&envelopes_inner);
                        handle_connection(stream, &requests_for_conn, &envelopes_for_conn, &routes);
                    }
                    Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            let _ = fs::remove_file(socket_path_inner);
        });

        Self {
            socket_path,
            stop,
            requests,
            envelopes,
            thread: Some(thread),
        }
    }

    pub(crate) fn requests_for_operation(&self, operation: &str) -> Vec<Value> {
        self.requests
            .lock()
            .expect("fake relay requests lock")
            .iter()
            .filter(|request| request.get("operation").and_then(Value::as_str) == Some(operation))
            .cloned()
            .collect::<Vec<_>>()
    }

    pub(crate) fn envelopes_for_operation(&self, operation: &str) -> Vec<Value> {
        self.envelopes
            .lock()
            .expect("fake relay envelopes lock")
            .iter()
            .filter(|envelope| {
                envelope
                    .get("request")
                    .and_then(|request| request.get("operation"))
                    .and_then(Value::as_str)
                    == Some(operation)
            })
            .cloned()
            .collect::<Vec<_>>()
    }
}

impl Drop for FakeRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle_connection(
    mut stream: UnixStream,
    requests: &Arc<Mutex<Vec<Value>>>,
    envelopes: &Arc<Mutex<Vec<Value>>>,
    routes: &Arc<HashMap<String, RelayResponder>>,
) {
    stream
        .set_nonblocking(false)
        .expect("set fake relay connection stream blocking");
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .expect("clone fake relay stream for reader"),
    );
    let mut bound_bundle: Option<String> = None;
    loop {
        let mut line = String::new();
        let bytes = match reader.read_line(&mut line) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(source) if source.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(source) => panic!("read fake relay request: {source:?}"),
        };
        if bytes == 0 {
            return;
        }
        let decoded: Value =
            serde_json::from_str(line.trim_end()).expect("decode fake relay request");
        if decoded.get("frame").and_then(Value::as_str) == Some("hello") {
            // A session principal_id is `<session>@<bundle>`; the routing bundle
            // is the namespace after the final `@`.
            let principal_id = decoded
                .get("principal_id")
                .and_then(Value::as_str)
                .expect("hello principal_id");
            let bundle_name = principal_id
                .rsplit_once('@')
                .map(|(_, namespace)| namespace.to_string())
                .expect("session principal_id must carry a bundle namespace");
            bound_bundle = Some(bundle_name);
            let hello_ack = json!({
                "frame": "hello_ack",
                "schema_version": decoded["schema_version"],
                "principal_id": decoded["principal_id"],
            });
            let text = serde_json::to_string(&hello_ack).expect("encode hello ack");
            stream
                .write_all(text.as_bytes())
                .expect("write fake relay hello ack");
            stream.write_all(b"\n").expect("write fake relay newline");
            stream.flush().expect("flush fake relay hello ack");
            continue;
        }
        let bound = bound_bundle
            .as_deref()
            .expect("fake relay received request before hello");
        // Per-request envelope namespace selects the routing bundle; absent
        // it the request lands on the Hello-bound bundle.
        let routing_namespace = decoded
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or(bound);
        let responder = routes.get(routing_namespace).unwrap_or_else(|| {
            panic!("fake relay missing responder for namespace {routing_namespace}")
        });
        if decoded.get("frame").and_then(Value::as_str) == Some("request") {
            let request = decoded
                .get("request")
                .cloned()
                .expect("stream request frame must include request");
            requests
                .lock()
                .expect("fake relay requests lock")
                .push(request.clone());
            envelopes
                .lock()
                .expect("fake relay envelopes lock")
                .push(decoded.clone());
            let response = responder(&request);
            let framed = json!({
                "frame": "response",
                "request_id": decoded.get("request_id").cloned().unwrap_or(Value::Null),
                "response": response,
            });
            let text = serde_json::to_string(&framed).expect("encode fake relay response");
            stream
                .write_all(text.as_bytes())
                .expect("write fake relay response");
            stream.write_all(b"\n").expect("write fake relay newline");
            stream.flush().expect("flush fake relay response");
            continue;
        }

        requests
            .lock()
            .expect("fake relay requests lock")
            .push(decoded.clone());
        let response = responder(&decoded);
        let text = serde_json::to_string(&response).expect("encode fake relay response");
        stream
            .write_all(text.as_bytes())
            .expect("write fake relay response");
        stream.write_all(b"\n").expect("write fake relay newline");
        stream.flush().expect("flush fake relay response");
    }
}

pub(crate) struct McpHarness {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
    instructions: String,
}

impl McpHarness {
    pub(crate) async fn spawn(runtime: &TestRuntime) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
        command
            .arg("host")
            .arg("mcp")
            .arg("--bundle")
            .arg(BUNDLE_NAME)
            .arg("--session-name")
            .arg(SENDER_SESSION)
            .arg("--configuration-directory")
            .arg(&runtime.config_root)
            .arg("--state-directory")
            .arg(&runtime.state_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        strip_bring_up_context(&mut command);

        let mut child = command.spawn().expect("spawn agentmux host mcp");
        let stdin = child.stdin.take().expect("take mcp stdin");
        let stdout = child.stdout.take().expect("take mcp stdout");
        let mut harness = Self {
            child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
            instructions: String::new(),
        };
        harness.initialize().await;
        harness
    }

    /// Spawns a fully associated MCP server carrying a retained startup fault.
    ///
    /// The fault is produced *after* roots and association resolve, by pointing
    /// the inscriptions directory at a path beneath a regular file so the sink
    /// cannot be created. That combination — roots present, association
    /// complete, readiness `Unavailable` — is the one a gate on association
    /// alone lets through, so it is the shape worth building a harness for.
    pub(crate) async fn spawn_with_retained_fault(runtime: &TestRuntime) -> Self {
        let blocker = runtime.root.join("inscriptions-blocker");
        std::fs::write(&blocker, b"not a directory").expect("write inscriptions blocker");

        let mut command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
        command
            .arg("host")
            .arg("mcp")
            .arg("--bundle")
            .arg(BUNDLE_NAME)
            .arg("--session-name")
            .arg(SENDER_SESSION)
            .arg("--configuration-directory")
            .arg(&runtime.config_root)
            .arg("--state-directory")
            .arg(&runtime.state_root)
            .arg("--inscriptions-directory")
            .arg(blocker.join("under-a-file"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        strip_bring_up_context(&mut command);

        let mut child = command.spawn().expect("spawn faulted agentmux host mcp");
        let stdin = child.stdin.take().expect("take mcp stdin");
        let stdout = child.stdout.take().expect("take mcp stdout");
        let mut harness = Self {
            child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
            instructions: String::new(),
        };
        harness.initialize().await;
        harness
    }

    /// Spawns a relay-wide (unassociated) MCP server: no `--bundle` or
    /// `--session-name`, so it carries no sender session and holds no relay
    /// stream. Used to prove relay-backed paths surface a typed
    /// `validation_unassociated_server` rather than an internal failure.
    pub(crate) async fn spawn_unassociated(runtime: &TestRuntime) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
        command
            .arg("host")
            .arg("mcp")
            .arg("--configuration-directory")
            .arg(&runtime.config_root)
            .arg("--state-directory")
            .arg(&runtime.state_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        // Unassociated is the whole point of this harness: inherited context
        // would silently associate it and void the test.
        strip_bring_up_context(&mut command);

        let mut child = command
            .spawn()
            .expect("spawn unassociated agentmux host mcp");
        let stdin = child.stdin.take().expect("take mcp stdin");
        let stdout = child.stdout.take().expect("take mcp stdout");
        let mut harness = Self {
            child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
            instructions: String::new(),
        };
        harness.initialize().await;
        harness
    }

    async fn initialize(&mut self) {
        let initialize =
            rmcp::model::InitializeRequest::new(rmcp::model::InitializeRequestParams::new(
                rmcp::model::ClientCapabilities::default(),
                rmcp::model::Implementation::new("agentmux-contract-tests", "0.0.0"),
            ));
        self.send(rmcp::model::ClientJsonRpcMessage::request(
            initialize.into(),
            rmcp::model::RequestId::Number(1),
        ))
        .await;
        let response = self.read_response(1).await;
        assert!(
            response.get("result").is_some(),
            "initialize response must contain result: {response}"
        );
        self.instructions = response["result"]["instructions"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let initialized = rmcp::model::InitializedNotification::default();
        self.send(rmcp::model::ClientJsonRpcMessage::notification(
            initialized.into(),
        ))
        .await;
    }

    /// The `instructions` string the server returned at `initialize`, captured
    /// from `ServerHandler::get_info`. Empty when the server sent none.
    pub(crate) fn instructions(&self) -> &str {
        &self.instructions
    }

    pub(crate) async fn list_tools(&mut self, id: i64) -> Value {
        let request = rmcp::model::ListToolsRequest::with_param(
            rmcp::model::PaginatedRequestParams::default(),
        );
        self.send(rmcp::model::ClientJsonRpcMessage::request(
            request.into(),
            rmcp::model::RequestId::Number(id),
        ))
        .await;
        self.read_response(id).await
    }

    pub(crate) async fn call_tool(
        &mut self,
        id: i64,
        name: &str,
        arguments: Map<String, Value>,
    ) -> Value {
        let request = rmcp::model::CallToolRequest::new(
            rmcp::model::CallToolRequestParams::new(name.to_string()).with_arguments(arguments),
        );
        self.send(rmcp::model::ClientJsonRpcMessage::request(
            request.into(),
            rmcp::model::RequestId::Number(id),
        ))
        .await;
        self.read_response(id).await
    }

    async fn send(&mut self, message: rmcp::model::ClientJsonRpcMessage) {
        let line = serde_json::to_string(&message).expect("encode mcp request");
        self.stdin
            .write_all(line.as_bytes())
            .await
            .expect("write mcp request");
        self.stdin
            .write_all(b"\n")
            .await
            .expect("write mcp newline");
        self.stdin.flush().await.expect("flush mcp request");
    }

    async fn read_response(&mut self, id: i64) -> Value {
        let expected = rmcp::model::RequestId::Number(id);
        let deadline = Instant::now() + READ_TIMEOUT;
        let mut line = String::new();
        loop {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for MCP response id {id}"
            );
            line.clear();
            let count = self
                .stdout
                .read_line(&mut line)
                .await
                .expect("read mcp response line");
            assert!(count > 0, "mcp process closed stdout");
            let decoded: Value =
                serde_json::from_str(line.trim_end()).expect("decode mcp response");
            let response_id = decoded.get("id").and_then(|id_value| {
                serde_json::from_value::<rmcp::model::RequestId>(id_value.clone()).ok()
            });
            if response_id == Some(expected.clone()) {
                return decoded;
            }
        }
    }
}

impl Drop for McpHarness {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

pub(crate) fn write_bundle_configuration(config_root: &Path, bundle_name: &str, sessions: &[&str]) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"
"#,
    )
    .expect("write policies config");

    let mut bundle = String::from("format-version = 1\n");
    for session in sessions {
        bundle.push_str(
            format!(
                "\n[[sessions]]\nid = \"{name}\"\nname = \"{name}\"\ndirectory = \"/tmp\"\ncoder = \"default\"\n",
                name = session
            )
            .as_str(),
        );
    }
    let path = config_root
        .join("bundles")
        .join(format!("{bundle_name}.toml"));
    fs::write(path, bundle).expect("write bundle config");
}

/// Default responder for the relay-wide `GLOBAL` namespace used by tests that
/// don't care about its content: returns a `down` bundle with no sessions, the
/// shape the real relay produces when no relay-wide principals are registered.
pub(crate) fn default_empty_global_responder() -> RelayResponder {
    Arc::new(
        |request| match request.get("operation").and_then(Value::as_str) {
            Some("list") => json!({
                "kind": "list",
                "schema_version": "1",
                "bundle": {
                    "id": "GLOBAL",
                    "hosted": false,
                    "state": "down",
                    "startup_health": null,
                    "state_reason_code": null,
                    "state_reason": null,
                    "startup_failure_count": 0,
                    "recent_startup_failures": [],
                    "principals": [],
                },
            }),
            _ => json!({
                "kind": "error",
                "error": {
                    "code": "internal_unexpected_failure",
                    "message": "unexpected operation",
                },
            }),
        },
    )
}

pub(crate) fn decode_tool_payload(response: &Value) -> Value {
    if let Some(payload) = response
        .get("result")
        .and_then(|result| result.get("structuredContent"))
        && !payload.is_null()
    {
        return payload.clone();
    }
    let content = response
        .get("result")
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .unwrap_or_else(|| panic!("missing result.content in response: {response}"));

    if let Some(json_payload) = content.get("json") {
        return json_payload.clone();
    }
    let text = content
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing content.text in response: {response}"));
    serde_json::from_str(text).expect("decode content.text as json")
}

pub(crate) fn error_code(response: &Value) -> Option<&str> {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

/// Asserts the response is the JSON-RPC `invalid_params` (-32602) rejection that
/// rmcp emits when a required parameter is missing or ill-typed, before the tool
/// handler runs. Required selectors are enforced by the tool input schema, so an
/// absent one surfaces here as a parameter-deserialization error naming `field`,
/// not as a handler-level `validation_invalid_params` code.
pub(crate) fn assert_param_deserialize_error(response: &Value, field: &str) {
    let error = response
        .get("error")
        .unwrap_or_else(|| panic!("expected protocol error in response: {response}"));
    assert_eq!(
        error.get("code").and_then(Value::as_i64),
        Some(-32602),
        "expected invalid_params (-32602) in response: {response}"
    );
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("missing error.message in response: {response}"));
    assert!(
        message.contains(field),
        "expected deserialize error naming '{field}', got: {message}"
    );
}

pub(crate) fn assert_unknown_field_error(response: &Value, expected_fields: &[&str]) {
    assert_eq!(error_code(response), Some("validation_invalid_params"));
    let fields = response["error"]["data"]["details"]["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("missing unknown-field details: {response}"))
        .iter()
        .map(|value| {
            value
                .as_str()
                .unwrap_or_else(|| panic!("unknown-field entry must be string: {response}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(fields, expected_fields);
}
