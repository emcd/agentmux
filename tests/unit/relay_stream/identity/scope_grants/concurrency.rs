use super::*;

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
    let blackhole = TmuxBlackhole::arm(&bundle_paths.runtime_directory);

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
        // A second operation on the same connection after the release must
        // also observe the replacement grant: namespace discovery hides the
        // no-longer-covered bundle as an empty success.
        send_json(
            &mut client,
            json!({
                "frame": "request",
                "request_id": "post-commit-namespaces",
                "request": {"operation": "discover_namespaces"},
            }),
        );
        let mut namespaces = read_json(&mut reader);
        while namespaces["frame"] != "response" {
            namespaces = read_json(&mut reader);
        }
        let _ = done_tx.send(namespaces);
        shutdown_stream(&client, "shutdown stalled peer stream");
        join.join().expect("join stalled peer thread");
    });

    // The accept proves the probe is in flight and stalled: only then may the
    // scope update run. It must complete promptly rather than wait out the
    // probe behind the shared serialization.
    blackhole.wait_for_dial(Duration::from_secs(15));
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
    blackhole.release();
    let decided = done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("stalled discovery never completed after release");
    assert_eq!(decided["response"]["kind"], "error",);
    assert_eq!(
        decided["response"]["error"]["code"], "authorization_forbidden",
        "post-commit decision must use the replacement grant: {decided:?}"
    );
    let namespaces = done_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("post-commit namespace discovery never completed");
    assert_eq!(namespaces["response"]["kind"], "discover_namespaces");
    assert!(
        namespaces["response"]["namespaces"]
            .as_array()
            .expect("namespaces array")
            .is_empty(),
        "both discovery operations observe the replacement: {namespaces:?}"
    );
    probe.join().expect("join probe thread");
}

/// Contention smoke coverage for a raced admission against a narrowing
/// update on the shared serialization.
///
/// This test pins request STARTS with a barrier but cannot pin the
/// commit/admission order: whichever side holds the shared lock first wins
/// outright, so both resolutions are asserted as valid outcome classes
/// (admission-before-update admits under the old grant; update-before-final
/// -admission denies under the replacement) with a consistent final grant.
/// Deterministic order coverage lives in the serial tests
/// (`live_narrowing_applies_on_the_same_connection_without_reconnect`,
/// `admitted_send_survives_narrowing_without_teardown`, and
/// `shared_serial_narrow_then_send_denies`), which fix each order exactly.
#[test]
fn raced_admission_and_narrowing_resolve_safely_smoke() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_raceorder";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let operator_id = global_user_id(bundle_name);
    write_multi_operator_users(&configuration_roots, std::slice::from_ref(&operator_id));
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "raceorder@RELAY";
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

    // Race a peer Send against a narrowing update on the shared
    // serialization: whichever commits first, both outcomes stay valid —
    // admission-before-update admits under the old grant, update-before-final
    // -admission denies under the replacement — and neither corrupts state.
    let barrier = Arc::new(Barrier::new(2));
    let send_context = Arc::clone(&context);
    let send_target = target.clone();
    let send_peer = peer_id.to_string();
    let send_psk = psk.clone();
    let send_barrier = Arc::clone(&barrier);
    let send_thread = thread::spawn(move || {
        let (mut client, join) = spawn_relay_connection_on_context(send_context);
        let read_stream = client.try_clone().expect("clone stream");
        let mut reader = BufReader::new(read_stream);
        send_json(
            &mut client,
            json!({
                "frame": "hello",
                "schema_version": "1",
                "principal_id": send_peer,
                "identity_token": send_psk,
            }),
        );
        assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
        send_barrier.wait();
        send_json(
            &mut client,
            json!({
                "frame": "request",
                "request_id": "raced-send",
                "request": {
                    "operation": "send",
                    "requester_session": send_peer,
                    "message": "raced admission",
                    "targets": [send_target],
                    "broadcast": false,
                },
            }),
        );
        let mut response = read_json(&mut reader);
        while response["frame"] != "response" {
            response = read_json(&mut reader);
        }
        shutdown_stream(&client, "shutdown raced peer stream");
        join.join().expect("join raced peer thread");
        response
    });

    barrier.wait();
    let narrowed = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    assert_eq!(
        narrowed["response"]["kind"], "change_scope",
        "narrowing must commit: {narrowed:?}"
    );
    let raced = send_thread.join().expect("join send thread");
    match &raced["response"]["kind"] {
        Value::String(kind) if kind == "send" => {}
        _ => assert_eq!(
            raced["response"]["error"]["code"], "authorization_forbidden",
            "raced send resolves denied only under the replacement: {raced:?}"
        ),
    }
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        "other",
        "narrowing must be the final grant"
    );
}

/// Deterministic update-before-final-admission on the shared serialization:
/// a committed narrowing denies a later send, and re-widening restores it,
/// all on one peer connection sharing the admin lock with the updater.
#[test]
fn shared_serial_narrow_then_send_denies() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_sharedserial";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let operator_id = global_user_id(bundle_name);
    write_multi_operator_users(&configuration_roots, std::slice::from_ref(&operator_id));
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "sharedserial@RELAY";
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
                "request_id": format!("sharedserial-{request_id}"),
                "request": request,
            }),
        );
        let mut response = read_json(&mut reader);
        while response["frame"] != "response" {
            response = read_json(&mut reader);
        }
        response
    };

    // Narrow first: the committed replacement governs the later admission.
    let narrowed = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope");
    let denied = peer_request(json!({
        "operation": "send",
        "requester_session": peer_id,
        "message": "after narrowing",
        "targets": [target],
        "broadcast": false,
    }));
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "{denied:?}"
    );

    // Re-widen: the same connection admits again with no reconnect.
    let widened = operator_request_on_context(
        &context,
        &operator_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "*"}),
    );
    assert_eq!(widened["response"]["kind"], "change_scope");
    let allowed = peer_request(json!({
        "operation": "send",
        "requester_session": peer_id,
        "message": "after widening",
        "targets": [target],
        "broadcast": false,
    }));
    assert_eq!(allowed["response"]["kind"], "send", "{allowed:?}");
    shutdown_stream(&client, "shutdown peer stream");
    join.join().expect("join peer thread");
}
