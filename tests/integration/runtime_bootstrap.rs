use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Barrier, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use agentmux::runtime::{
    bootstrap::{BootstrapOptions, bootstrap_relay},
    paths::{BundleRuntimePaths, ensure_bundle_runtime_directory},
};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientCapabilities, ClientJsonRpcMessage,
    Implementation, InitializeRequest, InitializeRequestParams, InitializedNotification, RequestId,
};
use serde_json::{Map, Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt},
    process::Command,
};

const READ_TIMEOUT: Duration = Duration::from_secs(10);

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
            fs::remove_file(&socket_path).expect("remove stale socket");
        }
        fs::create_dir_all(
            socket_path
                .parent()
                .expect("relay socket parent should exist"),
        )
        .expect("create relay socket parent");
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

struct McpHarness {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    stdout: tokio::io::BufReader<tokio::process::ChildStdout>,
    stderr: tokio::io::BufReader<tokio::process::ChildStderr>,
}

impl McpHarness {
    async fn spawn_with_environment(
        current_directory: &Path,
        arguments: &[&str],
        environment: &[(String, String)],
    ) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agentmux"));
        command
            .current_dir(current_directory)
            .arg("host")
            .arg("mcp")
            .args(arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (key, value) in environment {
            command.env(key, value);
        }

        let mut child = command.spawn().expect("spawn agentmux host mcp");
        let stdin = child.stdin.take().expect("take mcp stdin");
        let stdout = child.stdout.take().expect("take mcp stdout");
        let stderr = child.stderr.take().expect("take mcp stderr");
        let mut harness = Self {
            child,
            stdin,
            stdout: tokio::io::BufReader::new(stdout),
            stderr: tokio::io::BufReader::new(stderr),
        };
        harness.initialize().await;
        harness
    }

    async fn initialize(&mut self) {
        let initialize = InitializeRequest::new(InitializeRequestParams::new(
            ClientCapabilities::default(),
            Implementation::new("runtime-bootstrap-tests", "0.0.0"),
        ));
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

    async fn call_tool(&mut self, id: i64, name: &str, arguments: Map<String, Value>) -> Value {
        let request = CallToolRequest::new(
            CallToolRequestParams::new(name.to_string()).with_arguments(arguments),
        );
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
            if count == 0 {
                let mut stderr = String::new();
                self.stderr
                    .read_to_string(&mut stderr)
                    .await
                    .expect("read mcp stderr");
                panic!("mcp process closed stdout; stderr: {stderr}");
            }
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

fn write_bundle_configuration_with_directories(
    config_root: &Path,
    bundle_name: &str,
    sessions: &[(&str, &Path)],
) {
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
    for (session, directory) in sessions {
        bundle.push_str(
            format!(
                "\n[[sessions]]\nid = \"{name}\"\nname = \"{name}\"\ndirectory = \"{}\"\ncoder = \"default\"\n",
                directory.display(),
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

fn hook_git_environment() -> Vec<(String, String)> {
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = std::process::Command::new("git")
        .current_dir(repository_root)
        .args(["rev-parse", "--path-format=absolute", "--git-dir"])
        .output()
        .expect("resolve repository git directory");
    assert!(
        output.status.success(),
        "resolve repository git directory failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let git_directory = String::from_utf8_lossy(&output.stdout).trim().to_string();
    vec![
        ("GIT_DIR".to_string(), git_directory),
        (
            "GIT_WORK_TREE".to_string(),
            repository_root.display().to_string(),
        ),
    ]
}

#[test]
fn concurrent_bootstrap_spawns_single_relay() {
    const CLIENTS: usize = 4;

    let temporary = TempDir::new().expect("temporary");
    let paths = BundleRuntimePaths::resolve(temporary.path(), "party").expect("paths");
    ensure_bundle_runtime_directory(&paths).expect("runtime directory");

    let spawn_count = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(CLIENTS));
    let listener = Arc::new(Mutex::new(None::<UnixListener>));
    let options = BootstrapOptions {
        auto_start_relay: true,
        startup_timeout: Duration::from_secs(2),
    };

    let mut handles = Vec::new();
    for _ in 0..CLIENTS {
        let paths = paths.clone();
        let spawn_count = Arc::clone(&spawn_count);
        let barrier = Arc::clone(&barrier);
        let listener = Arc::clone(&listener);
        handles.push(thread::spawn(move || {
            barrier.wait();
            bootstrap_relay(&paths, options, || {
                if spawn_count.fetch_add(1, Ordering::SeqCst) == 0 {
                    let bound =
                        UnixListener::bind(&paths.relay_socket).expect("bind relay listener");
                    *listener.lock().expect("listener lock") = Some(bound);
                }
                Ok(())
            })
            .map(|_| ())
        }));
    }

    for handle in handles {
        handle
            .join()
            .expect("thread join")
            .expect("bootstrap should succeed");
    }

    assert_eq!(spawn_count.load(Ordering::SeqCst), 1);
    drop(listener.lock().expect("listener lock").take());
}

#[test]
fn bootstrap_removes_stale_socket_before_spawn() {
    let temporary = TempDir::new().expect("temporary");
    let paths = BundleRuntimePaths::resolve(temporary.path(), "party").expect("paths");
    ensure_bundle_runtime_directory(&paths).expect("runtime directory");
    fs::write(&paths.relay_socket, "stale").expect("write stale file");

    let options = BootstrapOptions {
        auto_start_relay: true,
        startup_timeout: Duration::from_secs(2),
    };
    let mut listener = None;

    let report = bootstrap_relay(&paths, options, || {
        assert!(
            !paths.relay_socket.exists(),
            "stale socket should be removed"
        );
        listener = Some(UnixListener::bind(&paths.relay_socket).expect("bind listener"));
        Ok(())
    })
    .expect("bootstrap should succeed");

    assert!(report.spawned_relay);
    drop(listener.take());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_auto_discovers_association_from_non_git_cwd() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("relay");
    let config_root = root.join("config");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");
    write_bundle_configuration(&config_root, "relay", &["relay", "bravo"]);

    let relay_socket = state_root.join("bundles/relay/relay.sock");
    let relay = FakeRelay::start(
        relay_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": "relay",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
                    "delivery_mode": request.get("delivery_mode").cloned().unwrap_or(Value::Null),
                    "status": "success",
                    "results": [{
                        "target_session": "bravo",
                        "message_id": "msg-1",
                        "outcome": "delivered",
                    }],
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

    let git_environment = hook_git_environment();
    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--config-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &git_environment,
    )
    .await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["sender_session"], "relay");
    assert_eq!(payload["status"], "success");

    let chat_requests = relay.requests_for_operation("chat");
    assert_eq!(chat_requests.len(), 1);
    assert_eq!(chat_requests[0]["sender_session"], "relay");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_falls_back_to_directory_match_when_auto_sender_is_not_member() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("master");
    let other = root.join("other");
    let config_root = root.join("config");
    let state_root = root.join("state");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&other).expect("create other");
    write_bundle_configuration_with_directories(
        &config_root,
        "master",
        &[("coordinator", &workspace), ("bravo", &other)],
    );

    let relay_socket = state_root.join("bundles/master/relay.sock");
    let relay = FakeRelay::start(
        relay_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("chat") => json!({
                    "kind": "chat",
                    "schema_version": "1",
                    "bundle_name": "master",
                    "request_id": request.get("request_id").cloned().unwrap_or(Value::Null),
                    "sender_session": request.get("sender_session").cloned().unwrap_or(Value::Null),
                    "delivery_mode": request.get("delivery_mode").cloned().unwrap_or(Value::Null),
                    "status": "success",
                    "results": [{
                        "target_session": "bravo",
                        "message_id": "msg-1",
                        "outcome": "delivered",
                    }],
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

    let git_environment = hook_git_environment();
    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--config-directory",
            config_root.to_str().expect("utf8 config path"),
            "--state-directory",
            state_root.to_str().expect("utf8 state path"),
        ],
        &git_environment,
    )
    .await;

    let mut arguments = Map::new();
    arguments.insert("message".to_string(), Value::String("hello".to_string()));
    arguments.insert(
        "targets".to_string(),
        Value::Array(vec![Value::String("bravo".to_string())]),
    );
    arguments.insert("broadcast".to_string(), Value::Bool(false));
    let response = harness.call_tool(2, "send", arguments).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["sender_session"], "coordinator");
    assert_eq!(payload["status"], "success");

    let chat_requests = relay.requests_for_operation("chat");
    assert_eq!(chat_requests.len(), 1);
    assert_eq!(chat_requests[0]["sender_session"], "coordinator");
}

#[cfg(debug_assertions)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_uses_repository_root_debug_state_override() {
    let temporary = TempDir::new().expect("temporary");
    let root = temporary.path().to_path_buf();
    let workspace = root.join("workspace");
    let repository_root = root.join("repository");
    let config_root = root.join("config");
    fs::create_dir_all(&workspace).expect("create workspace");
    fs::create_dir_all(&repository_root).expect("create repository root");
    write_bundle_configuration(&config_root, "party", &["alpha", "bravo"]);

    let relay_socket = repository_root
        .join(".auxiliary/state/agentmux")
        .join("bundles/party/relay.sock");
    let relay = FakeRelay::start(
        relay_socket,
        Arc::new(
            |request| match request.get("operation").and_then(Value::as_str) {
                Some("list") => json!({
                    "kind": "list",
                    "schema_version": "1",
                    "bundle_name": "party",
                    "recipients": [{"session_name": "bravo"}],
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

    let mut harness = McpHarness::spawn_with_environment(
        &workspace,
        &[
            "--bundle-name",
            "party",
            "--session-name",
            "alpha",
            "--config-directory",
            config_root.to_str().expect("utf8 config path"),
            "--repository-root",
            repository_root.to_str().expect("utf8 repository path"),
        ],
        &[],
    )
    .await;

    let response = harness.call_tool(2, "list", Map::new()).await;
    let payload = decode_tool_payload(&response);
    assert_eq!(payload["bundle_name"], "party");
    assert_eq!(relay.requests_for_operation("list").len(), 1);
}
