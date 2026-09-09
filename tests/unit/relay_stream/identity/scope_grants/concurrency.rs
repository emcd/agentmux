use super::*;

/// Writes a users.toml declaring `operators` distinct `@GLOBAL` operators on
/// the operator policy, so concurrent admin callers on one shared context do
/// not collide on a single identity claim.
fn write_multi_operator_users(configuration_roots: &ConfigurationRoots, operators: &[String]) {
    let sessions = operators
        .iter()
        .map(|operator| {
            format!("[[sessions]]\nid = \"{operator}\"\npolicy = \"operator\"\n\n[sessions.ui]\n")
        })
        .collect::<String>();
    std::fs::write(
        configuration_roots.base_layer().join("users.toml"),
        format!("default-session = \"{}\"\n\n{}", operators[0], sessions),
    )
    .expect("write multi-operator users configuration");
}

#[test]
fn concurrent_scope_updates_and_rotation_serialize_on_shared_context() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_race";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let operators: Vec<String> = (1..=4)
        .map(|index| format!("raceop{index}@GLOBAL"))
        .collect();
    write_multi_operator_users(&configuration_roots, &operators);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "raced@RELAY";
    let catalog = single_bundle_catalog(&bundle_paths);
    let context = shared_serve_context(&configuration_roots, &state_root, catalog);

    // Seed the peer serially so every racer starts from a known record.
    let seed = operator_request_on_context(
        &context,
        &operators[0],
        json!({"operation": "new_peer", "principal_id": peer_id, "scope": "*"}),
    );
    assert_eq!(seed["response"]["kind"], "new_peer", "seed: {seed:?}");

    // Fire narrowing, widening, rotation, and an unrelated registration so
    // they contend on the shared identity-admin serialization as closely as
    // the barrier allows.
    let barrier = Arc::new(Barrier::new(4));
    let spawn_admin = |operator: String, request: Value| {
        let context = Arc::clone(&context);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            operator_request_on_context(&context, &operator, request)
        })
    };
    let narrow = spawn_admin(
        operators[0].clone(),
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": bundle_name}),
    );
    let widen = spawn_admin(
        operators[1].clone(),
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "*"}),
    );
    let rotate = spawn_admin(
        operators[2].clone(),
        json!({"operation": "change_psk", "principal_id": peer_id}),
    );
    let unrelated = spawn_admin(
        operators[3].clone(),
        json!({"operation": "new_peer", "principal_id": format!("bystander@{bundle_name}")}),
    );

    let narrowed = narrow.join().expect("join narrow thread");
    let widened = widen.join().expect("join widen thread");
    let rotation = rotate.join().expect("join rotate thread");
    let registration = unrelated.join().expect("join register thread");
    assert_eq!(narrowed["response"]["kind"], "change_scope", "{narrowed:?}");
    assert_eq!(widened["response"]["kind"], "change_scope", "{widened:?}");
    assert_eq!(rotation["response"]["kind"], "change_psk", "{rotation:?}");
    assert_eq!(
        registration["response"]["kind"], "new_peer",
        "{registration:?}"
    );

    // Last-writer-wins for the grant, no lost fields: the final scope is one
    // of the two committed replacements, the rotated credential authenticates,
    // and the unrelated registration survived.
    let record = store_record(&bundle_paths, peer_id);
    assert!(
        record["scope"] == bundle_name || record["scope"] == "*",
        "final scope must be a committed replacement: {record}"
    );
    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("rotated psk")
        .to_string();
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &rotated_psk,
        true,
    );
    assert_eq!(accepted["frame"], "hello_ack", "rotated psk: {accepted:?}");
}

#[test]
fn admitted_send_survives_narrowing_without_teardown() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_admitted";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let operator_id = global_user_id(bundle_name);
    write_multi_operator_users(&configuration_roots, std::slice::from_ref(&operator_id));
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "admitted@RELAY";
    let target = format!("alpha@{bundle_name}");
    let catalog = single_bundle_catalog(&bundle_paths);
    let context = shared_serve_context(&configuration_roots, &state_root, catalog);

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
    let hash_before = store_record(&bundle_paths, peer_id)["credential_hash"].clone();

    // One peer connection for the whole sequence: admission, narrowing, and
    // re-widening all happen without reconnecting or rotating.
    let (mut client, join) = spawn_relay_connection_on_context(Arc::clone(&context));
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
    let mut request_id = 0;
    let mut peer_request = |request: Value| -> Value {
        request_id += 1;
        send_json(
            &mut client,
            json!({
                "frame": "request",
                "request_id": format!("admitted-{request_id}"),
                "request": request,
            }),
        );
        let mut response = read_json(&mut reader);
        while response["frame"] != "response" {
            response = read_json(&mut reader);
        }
        response
    };

    // Admission first: the send is queued under the wildcard grant.
    let admitted = peer_request(json!({
        "operation": "send",
        "requester_session": peer_id,
        "message": "admitted before narrowing",
        "targets": [target],
        "broadcast": false,
    }));
    assert_eq!(admitted["response"]["kind"], "send", "{admitted:?}");

    // Narrowing commits afterwards: it governs subsequent admissions but
    // cannot retroactively cancel the admitted delivery or tear down the
    // connection, and it preserves the credential.
    let narrowed = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope", "{narrowed:?}");
    assert_eq!(
        store_record(&bundle_paths, peer_id)["credential_hash"],
        hash_before,
        "narrowing must preserve the credential"
    );

    // The same connection keeps working once the grant widens again: nothing
    // was torn down and no rotation happened.
    let widened = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "*"}),
    );
    assert_eq!(widened["response"]["kind"], "change_scope");
    let restored = peer_request(json!({
        "operation": "send",
        "requester_session": peer_id,
        "message": "after re-widening",
        "targets": [target],
        "broadcast": false,
    }));
    assert_eq!(
        restored["response"]["kind"], "send",
        "connection must survive narrowing: {restored:?}"
    );
    shutdown_stream(&client, "shutdown peer stream");
    join.join().expect("join peer thread");
}

#[test]
fn scope_update_progresses_while_a_principal_probe_is_stalled() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_stall";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let operator_id = global_user_id(bundle_name);
    write_multi_operator_users(&configuration_roots, std::slice::from_ref(&operator_id));
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "stalled@RELAY";
    let catalog = single_bundle_catalog(&bundle_paths);
    let context = shared_serve_context(&configuration_roots, &state_root, catalog);

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

    // Black-hole the bundle's tmux socket: the tmux CLI dials it during the
    // readiness probe of principal discovery and then waits indefinitely, so
    // the probe stays stalled until the test releases it.
    std::fs::create_dir_all(&bundle_paths.runtime_directory).expect("runtime dir");
    let socket_path = bundle_paths.runtime_directory.join("tmux.sock");
    if socket_path.exists() {
        std::fs::remove_file(&socket_path).expect("clear tmux socket path");
    }
    let listener = UnixListener::bind(&socket_path).expect("bind black-hole socket");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let (accepted_tx, accepted_rx) = mpsc::channel::<()>();
    let accepted_streams: Arc<Mutex<Vec<UnixStream>>> = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let accept_thread = {
        let listener = listener;
        let accepted_streams = Arc::clone(&accepted_streams);
        let stop = Arc::clone(&stop);
        let mut signaled = false;
        thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        accepted_streams.lock().expect("lock streams").push(stream);
                        if !signaled {
                            signaled = true;
                            let _ = accepted_tx.send(());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => break,
                }
            }
        })
    };

    // Start principal discovery on a shared-context peer connection: it will
    // stall inside the readiness probe.
    let (mut client, join) = spawn_relay_connection_on_context(Arc::clone(&context));
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
    let (done_tx, done_rx) = mpsc::channel::<Value>();
    let probe = thread::spawn(move || {
        send_json(
            &mut client,
            json!({
                "frame": "request",
                "request_id": "stalled-discovery",
                "request": {"operation": "discover_principals", "namespace": bundle_name},
            }),
        );
        let mut response = read_json(&mut reader);
        while response["frame"] != "response" {
            response = read_json(&mut reader);
        }
        let _ = done_tx.send(response);
        shutdown_stream(&client, "shutdown stalled peer stream");
        join.join().expect("join stalled peer thread");
    });

    // The accept proves the probe is in flight and stalled: only then may the
    // scope update run. It must complete promptly rather than wait out the
    // probe behind the shared serialization.
    accepted_rx
        .recv_timeout(Duration::from_secs(15))
        .expect("tmux probe never dialed the black-hole socket");
    let started = Instant::now();
    let narrowed = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    let elapsed = started.elapsed();
    assert_eq!(narrowed["response"]["kind"], "change_scope", "{narrowed:?}");
    assert!(
        elapsed < Duration::from_secs(10),
        "scope update blocked {elapsed:?} on a stalled probe"
    );
    assert!(
        done_rx.try_recv().is_err(),
        "discovery must still be stalled in its probe"
    );

    // Release the probe: the tmux clients fail, the decision resolves under
    // the replacement grant committed above, and the narrowed scope denies a
    // request that started before the commit.
    stop.store(true, Ordering::SeqCst);
    accept_thread.join().expect("join accept thread");
    for stream in accepted_streams.lock().expect("lock streams").drain(..) {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
    let decided = done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("stalled discovery never completed after release");
    assert_eq!(decided["response"]["kind"], "error");
    assert_eq!(
        decided["response"]["error"]["code"], "authorization_forbidden",
        "post-commit decision must use the replacement grant: {decided:?}"
    );
    probe.join().expect("join probe thread");
}
