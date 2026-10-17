use super::*;

/// A real tmux server held stopped/running by signals, so target execution
/// can be held at a deterministic point and released on demand. Drop kills
/// the server unconditionally, so a stopped server never leaks.
struct HeldTmuxServer {
    server_pid: u32,
}

impl HeldTmuxServer {
    fn start(socket: &std::path::Path, session: &str) -> Self {
        assert!(
            socket.as_os_str().len() < 100,
            "tmux socket path too long for portability: {}",
            socket.display()
        );
        let status = std::process::Command::new("tmux")
            .args(["-S", &socket.to_string_lossy(), "kill-server"])
            .output();
        let _ = status;
        let create = std::process::Command::new("tmux")
            .args([
                "-S",
                &socket.to_string_lossy(),
                "new-session",
                "-d",
                "-s",
                session,
                "-x",
                "200",
                "-y",
                "50",
                "sleep",
                "120",
            ])
            .output()
            .expect("start tmux server");
        assert!(
            create.status.success(),
            "tmux new-session failed: {create:?}"
        );
        // Resolve the server pid (the client above already exited) and wait
        // until the session answers, proving the server is up.
        let server_pid = Self::query_pid(socket);
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if Instant::now() > deadline {
                panic!("tmux server never became ready");
            }
            let probe = std::process::Command::new("tmux")
                .args([
                    "-S",
                    &socket.to_string_lossy(),
                    "has-session",
                    "-t",
                    session,
                ])
                .output()
                .expect("probe tmux session");
            if probe.status.success() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        Self { server_pid }
    }

    fn query_pid(socket: &std::path::Path) -> u32 {
        let output = std::process::Command::new("tmux")
            .args([
                "-S",
                &socket.to_string_lossy(),
                "display-message",
                "-p",
                "#{pid}",
            ])
            .output()
            .expect("query tmux server pid");
        assert!(output.status.success(), "tmux pid query failed: {output:?}");
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("parse tmux server pid")
    }

    fn stop(&self) {
        let result = unsafe {
            libc::kill(self.server_pid as libc::pid_t, libc::SIGSTOP)
        };
        assert_eq!(result, 0, "SIGSTOP tmux server failed");
    }

    fn cont(&self) {
        let result = unsafe {
            libc::kill(self.server_pid as libc::pid_t, libc::SIGCONT)
        };
        assert_eq!(result, 0, "SIGCONT tmux server failed");
    }

    fn capture(&self, socket: &std::path::Path, session: &str) -> String {
        let output = std::process::Command::new("tmux")
            .args([
                "-S",
                &socket.to_string_lossy(),
                "capture-pane",
                "-p",
                "-t",
                session,
            ])
            .output()
            .expect("capture tmux pane");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for HeldTmuxServer {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.server_pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// Execution-timing proof that narrowing never retroactively cancels
/// admitted work: a real tmux server is held stopped so the delivery worker
/// cannot complete before the narrowing commits; after release, the original
/// message text is observed in the target pane. Runs on the shared context
/// throughout. Delivery before the narrowing completes is impossible here —
/// every tmux client blocks on the stopped server — so any observed delivery
/// necessarily executed after the commit.
#[test]
fn held_tmux_delivery_runs_after_narrowing_with_original_text() {
    // Scratch space under /tmp keeps the tmux socket path portable: unix
    // socket paths must fit sun_path (104 bytes on macOS), which
    // runner-controlled TMPDIR layouts can exceed, failing the bind with
    // "file name too long" for reasons unrelated to the behavior under test.
    let temporary = TempDir::new_in("/tmp").expect("short scratch space");
    let bundle_name = "ident_scope_tmuxhold";
    let configuration_roots =
        write_scope_configuration(&temporary, bundle_name);
    let operator_id = global_user_id(bundle_name);
    write_multi_operator_users(
        &configuration_roots,
        std::slice::from_ref(&operator_id),
    );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name)
        .expect("bundle paths");
    let peer_id = "tmuxhold@RELAY";
    let target = format!("alpha@{bundle_name}");
    let catalog = single_bundle_catalog(&bundle_paths);
    let context =
        shared_serve_context(&configuration_roots, &state_root, catalog);

    let seed = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "new_peer", "principal_id": peer_id, "scope": "*"}),
    );
    assert_eq!(seed["response"]["kind"], "new_peer");
    let psk = seed["response"]["psk"]
        .as_str()
        .expect("seed psk")
        .to_string();

    std::fs::create_dir_all(&bundle_paths.runtime_directory)
        .expect("runtime dir");
    let socket_path = bundle_paths.runtime_directory.join("tmux.sock");
    let server = HeldTmuxServer::start(&socket_path, "alpha");
    server.stop();

    let (mut client, join) =
        spawn_relay_connection_on_context(Arc::clone(&context));
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": peer_id,
            "identity_token": psk,
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    // Distinctive body so the pane observation matches this admission only.
    let body = format!("held-execution-{}", bundle_name);
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "held-tmux-send",
            "request": {
                "operation": "send",
                "requester_session": peer_id,
                "message": body,
                "targets": [target],
                "broadcast": false,
            },
        }),
    );
    let mut admitted = read_json(&mut reader);
    while admitted["frame"] != "response" {
        admitted = read_json(&mut reader);
    }
    assert_eq!(admitted["response"]["kind"], "send", "{admitted:?}");
    assert!(
        admitted["response"]["results"][0]["message_id"]
            .as_str()
            .is_some(),
        "admission must carry a message id: {admitted:?}"
    );

    // The worker cannot complete while the server is stopped, so the
    // narrowing below necessarily commits before any delivery executes.
    let narrowed = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope", "{narrowed:?}");
    server.cont();

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if Instant::now() > deadline {
            panic!("held delivery never reached the pane after release");
        }
        if server
            .capture(&socket_path, "alpha")
            .contains(body.as_str())
        {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        "other",
        "narrowing must be the final grant"
    );
    shutdown_stream(&client, "shutdown peer stream");
    join.join().expect("join peer thread");
}
