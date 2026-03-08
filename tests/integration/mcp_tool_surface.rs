use std::{
    collections::BTreeSet,
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use rmcp::model::{
    CallToolRequest, CallToolRequestParam, ClientCapabilities, ClientJsonRpcMessage,
    Implementation, InitializeRequest, InitializeRequestParam, InitializedNotification,
    ListToolsRequest, PaginatedRequestParam, RequestId,
};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::Command,
};

const READ_TIMEOUT: Duration = Duration::from_secs(10);
const BUNDLE_NAME: &str = "party";
const SENDER_SESSION: &str = "alpha";

struct TestRuntime {
    root: PathBuf,
    config_root: PathBuf,
    state_root: PathBuf,
    relay_socket: PathBuf,
}

impl TestRuntime {
    fn create() -> Self {
        let root = temporary_root("mcp-tool-surface");
        let config_root = root.join("config");
        let state_root = root.join("state");
        let relay_socket = state_root
            .join("bundles")
            .join(BUNDLE_NAME)
            .join("relay.sock");

        fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
        fs::create_dir_all(
            relay_socket
                .parent()
                .expect("relay socket parent should exist"),
        )
        .expect("create relay socket parent");
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
        }
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

type RelayResponder = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

struct FakeRelay {
    socket_path: PathBuf,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Value>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FakeRelay {
    fn start(socket_path: PathBuf, responder: RelayResponder) -> Self {
        if socket_path.exists() {
            fs::remove_file(&socket_path).expect("remove stale relay socket");
        }
        let listener = UnixListener::bind(&socket_path).expect("bind fake relay");
        listener
            .set_nonblocking(true)
            .expect("set fake relay listener nonblocking");

        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop_inner = Arc::clone(&stop);
        let requests_inner = Arc::clone(&requests);
        let socket_path_inner = socket_path.clone();

        let thread = thread::spawn(move || {
            while !stop_inner.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _address)) => {
                        handle_connection(stream, &requests_inner, &responder);
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
            thread: Some(thread),
        }
    }

    fn requests_for_operation(&self, operation: &str) -> Vec<Value> {
        self.requests
            .lock()
            .expect("fake relay requests lock")
            .iter()
            .filter(|request| request.get("operation").and_then(Value::as_str) == Some(operation))
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
    responder: &RelayResponder,
) {
    let mut line = String::new();
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .expect("clone fake relay stream for reader"),
    );
    let bytes = reader
        .read_line(&mut line)
        .expect("read fake relay request");
    if bytes == 0 {
        return;
    }
    let request: Value = serde_json::from_str(line.trim_end()).expect("decode fake relay request");
    requests
        .lock()
        .expect("fake relay requests lock")
        .push(request.clone());
    let response = responder(&request);
    let text = serde_json::to_string(&response).expect("encode fake relay response");
    stream
        .write_all(text.as_bytes())
        .expect("write fake relay response");
    stream.write_all(b"\n").expect("write fake relay newline");
    stream.flush().expect("flush fake relay response");
}

struct McpHarness {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
}

impl McpHarness {
    async fn spawn(runtime: &TestRuntime) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
        command
            .arg("host")
            .arg("mcp")
            .arg("--bundle")
            .arg(BUNDLE_NAME)
            .arg("--session-name")
            .arg(SENDER_SESSION)
            .arg("--config-directory")
            .arg(&runtime.config_root)
            .arg("--state-directory")
            .arg(&runtime.state_root)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = command.spawn().expect("spawn agentmux host mcp");
        let stdin = child.stdin.take().expect("take mcp stdin");
        let stdout = child.stdout.take().expect("take mcp stdout");
        let mut harness = Self {
            child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
        };
        harness.initialize().await;
        harness
    }

    async fn initialize(&mut self) {
        let initialize = InitializeRequest::new(InitializeRequestParam {
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "agentmux-contract-tests".to_string(),
                title: None,
                version: "0.0.0".to_string(),
                icons: None,
                website_url: None,
            },
        });
        self.send(ClientJsonRpcMessage::request(
            initialize.into(),
            RequestId::Number(1),
        ))
        .await;
        let response = self.read_response(1).await;
        assert!(
            response.get("result").is_some(),
            "initialize response must contain result: {response}"
        );

        let initialized = InitializedNotification::default();
        self.send(ClientJsonRpcMessage::notification(initialized.into()))
            .await;
    }

    async fn list_tools(&mut self, id: i64) -> Value {
        let request = ListToolsRequest::with_param(PaginatedRequestParam { cursor: None });
        self.send(ClientJsonRpcMessage::request(
            request.into(),
            RequestId::Number(id),
        ))
        .await;
        self.read_response(id).await
    }

    async fn call_tool(&mut self, id: i64, name: &str, arguments: Map<String, Value>) -> Value {
        let request = CallToolRequest::new(CallToolRequestParam {
            name: name.to_string().into(),
            arguments: Some(arguments),
        });
        self.send(ClientJsonRpcMessage::request(
            request.into(),
            RequestId::Number(id),
        ))
        .await;
        self.read_response(id).await
    }

    async fn send(&mut self, message: ClientJsonRpcMessage) {
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
        let expected = RequestId::Number(id);
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
            let response_id = decoded
                .get("id")
                .and_then(|id_value| serde_json::from_value::<RequestId>(id_value.clone()).ok());
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

fn write_bundle_configuration(config_root: &Path, bundle_name: &str, sessions: &[&str]) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "default"
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");

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

fn temporary_root(prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = PathBuf::from(".auxiliary/temporary").join(format!("{prefix}-{pid}-{nanos}"));
    fs::create_dir_all(&root).expect("create temporary root");
    root
}

fn decode_tool_payload(response: &Value) -> Value {
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

fn error_code(response: &Value) -> Option<&str> {
    response
        .get("error")
        .and_then(|error| error.get("data"))
        .and_then(|data| data.get("code"))
        .and_then(Value::as_str)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tool_catalog_contains_list_and_send() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "recipients": [],
                }),
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": SENDER_SESSION,
                    "delivery_mode": "sync",
                    "status": "success",
                    "results": [],
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.list_tools(2).await;
    let tools = response["result"]["tools"]
        .as_array()
        .expect("tools list array");
    let names = tools
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        names,
        BTreeSet::from(["list".to_string(), "send".to_string()])
    );

    assert!(relay.requests_for_operation("list").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_returns_recipient_payload_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "recipients": [
                        {"session_name": "bravo", "display_name": "Bravo"},
                        {"session_name": "charlie"},
                    ],
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "list", Map::new()).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["schema_version"], "1");
    assert_eq!(payload["bundle_name"], BUNDLE_NAME);
    assert_eq!(payload["recipients"][0]["session_name"], "bravo");
    assert_eq!(payload["recipients"][0]["display_name"], "Bravo");
    assert_eq!(payload["recipients"][1]["session_name"], "charlie");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_reports_relay_unavailable_when_relay_is_not_running() {
    let runtime = TestRuntime::create();
    let mut harness = McpHarness::spawn(&runtime).await;
    let response = harness.call_tool(2, "list", Map::new()).await;

    assert_eq!(error_code(&response), Some("relay_unavailable"));
    assert_eq!(
        response["error"]["data"]["details"]["relay_socket"],
        Value::String(runtime.relay_socket.display().to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_conflicting_targets_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(true));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(
        error_code(&response),
        Some("validation_conflicting_targets")
    );
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_empty_targets_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert("targets".to_string(), Value::Array(Vec::new()));
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("validation_empty_targets"));
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_empty_message_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("   ".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("validation_invalid_arguments"));
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_rejects_invalid_quiescence_timeout_before_relay_request() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(|_| panic!("relay should not receive chat request for invalid parameters")),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    arguments.insert("quiescence_timeout_ms".to_string(), Value::Number(0.into()));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(
        error_code(&response),
        Some("validation_invalid_quiescence_timeout")
    );
    assert!(relay.requests_for_operation("chat").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_returns_partial_and_forwards_sender_session() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
                    "sender_display_name": "Alpha",
                    "delivery_mode": request.get("delivery_mode").cloned().unwrap_or(Value::Null),
                    "status": "partial",
                    "results": [
                        {
                            "target_session": "bravo",
                            "message_id": "msg-1",
                            "outcome": "delivered",
                        },
                        {
                            "target_session": "charlie",
                            "message_id": "msg-2",
                            "outcome": "timeout",
                            "reason": "delivery_quiescence_timeout",
                        }
                    ],
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert("request_id".to_string(), Value::String("req-7".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![
            Value::String("bravo".to_string()),
            Value::String("charlie".to_string()),
        ]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);

    assert_eq!(payload["status"], "partial");
    assert_eq!(payload["sender_session"], SENDER_SESSION);
    assert_eq!(payload["sender_display_name"], "Alpha");
    assert_eq!(payload["delivery_mode"], "async");
    assert_eq!(payload["results"][1]["outcome"], "timeout");
    assert_eq!(
        payload["results"][1]["reason"],
        "delivery_quiescence_timeout"
    );

    let relay_requests = relay.requests_for_operation("chat");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["sender_session"], SENDER_SESSION);
    assert_eq!(relay_requests[0]["targets"][0], "bravo");
    assert_eq!(relay_requests[0]["targets"][1], "charlie");
    assert_eq!(relay_requests[0]["delivery_mode"], "async");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_forwards_sync_mode_and_timeout_override() {
    let runtime = TestRuntime::create();
    let relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": BUNDLE_NAME,
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
                    "delivery_mode": request.get("delivery_mode").cloned().unwrap_or(Value::Null),
                    "status": "success",
                    "results": [],
                }),
                _ => json!({
                    "kind": "error",
                    "error": {
                        "code": "internal_unexpected_failure",
                        "message": "unexpected operation",
                    },
                }),
            },
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    arguments.insert(
        "delivery_mode".to_string(),
        Value::String("sync".to_string()),
    );
    arguments.insert(
        "quiescence_timeout_ms".to_string(),
        Value::Number(1234.into()),
    );
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["delivery_mode"], "sync");

    let relay_requests = relay.requests_for_operation("chat");
    assert_eq!(relay_requests.len(), 1);
    assert_eq!(relay_requests[0]["delivery_mode"], "sync");
    assert_eq!(relay_requests[0]["quiescence_timeout_ms"], 1234);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_maps_unknown_sender_error_from_relay() {
    let runtime = TestRuntime::create();
    let _relay = FakeRelay::start(
        runtime.relay_socket.clone(),
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "error",
                    "error": {
                        "code": "validation_unknown_sender",
                        "message": "sender_session is not in bundle configuration",
                        "details": {"sender_session": SENDER_SESSION},
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
        ),
    );
    let mut harness = McpHarness::spawn(&runtime).await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;

    assert_eq!(error_code(&response), Some("validation_unknown_sender"));
}
