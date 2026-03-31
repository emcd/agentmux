use std::{
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

use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt},
    process::Command,
};

pub(crate) const READ_TIMEOUT: Duration = Duration::from_secs(10);
pub(crate) const BUNDLE_NAME: &str = "party";
pub(crate) const SENDER_SESSION: &str = "alpha";

pub(crate) type RelayResponder = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

pub(crate) struct TestRuntime {
    pub root: PathBuf,
    pub config_root: PathBuf,
    pub state_root: PathBuf,
    pub relay_socket: PathBuf,
}

impl TestRuntime {
    pub(crate) fn create() -> Self {
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

pub(crate) struct FakeRelay {
    socket_path: PathBuf,
    stop: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<Value>>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FakeRelay {
    pub(crate) fn start(socket_path: PathBuf, responder: RelayResponder) -> Self {
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

    pub(crate) fn requests_for_operation(&self, operation: &str) -> Vec<Value> {
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
    stream
        .set_nonblocking(false)
        .expect("set fake relay connection stream blocking");
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .expect("clone fake relay stream for reader"),
    );
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
            let hello_ack = json!({
                "frame": "hello_ack",
                "schema_version": decoded["schema_version"],
                "bundle_name": decoded["bundle_name"],
                "session_id": decoded["session_id"],
                "client_class": decoded["client_class"],
            });
            let text = serde_json::to_string(&hello_ack).expect("encode hello ack");
            stream
                .write_all(text.as_bytes())
                .expect("write fake relay hello ack");
            stream.write_all(b"\n").expect("write fake relay newline");
            stream.flush().expect("flush fake relay hello ack");
            continue;
        }
        if decoded.get("frame").and_then(Value::as_str) == Some("request") {
            let request = decoded
                .get("request")
                .cloned()
                .expect("stream request frame must include request");
            requests
                .lock()
                .expect("fake relay requests lock")
                .push(request.clone());
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

        let initialized = rmcp::model::InitializedNotification::default();
        self.send(rmcp::model::ClientJsonRpcMessage::notification(
            initialized.into(),
        ))
        .await;
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
find = "self"
list = "all:home"
look = "self"
send = "all:home"
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

pub(crate) fn temporary_root(prefix: &str) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = PathBuf::from(".auxiliary/temporary").join(format!("{prefix}-{pid}-{nanos}"));
    fs::create_dir_all(&root).expect("create temporary root");
    root
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
