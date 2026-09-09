use super::*;

#[test]
fn change_scope_updates_grant_and_returns_canonical() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_update";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "update@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    let hash_before = store_record(&bundle_paths, peer_id)["credential_hash"].clone();

    let updated = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(" zeta,alpha "),
    );
    assert_eq!(
        updated["response"]["kind"], "change_scope",
        "change scope rejected: {updated:?}"
    );
    assert_eq!(updated["response"]["scope"], "alpha,zeta");

    let record = store_record(&bundle_paths, peer_id);
    assert_eq!(record["scope"], "alpha,zeta");
    assert_eq!(
        record["credential_hash"], hash_before,
        "scope replacement must preserve the credential: {record}"
    );
}

#[test]
fn change_scope_empty_clears_grant_and_repeats_safely() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_clear";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "clear@RELAY";

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    for _ in 0..2 {
        let cleared = change_scope_request(
            &configuration_roots,
            &bundle_paths,
            bundle_name,
            peer_id,
            Some(""),
        );
        assert_eq!(
            cleared["response"]["kind"], "change_scope",
            "clear rejected: {cleared:?}"
        );
        assert_eq!(cleared["response"]["scope"], "");
    }
    assert!(
        store_record(&bundle_paths, peer_id).get("scope").is_none(),
        "cleared scope must persist absent"
    );

    let denied = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &psk,
        &format!("alpha@{bundle_name}"),
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "cleared peer must lose ingress rights: {denied:?}"
    );
}

#[test]
fn change_scope_requires_dedicated_authority() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_auth";
    // The shared identity configuration grants change.psk but not
    // change.scope: rotation rights must not confer scope administration.
    let configuration_roots = write_identity_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "auth@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );

    let denied = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "rotation-only caller must not change scope: {denied:?}"
    );
    assert_eq!(
        denied["response"]["error"]["details"]["capability"], "change.scope",
        "denial must identify the scope capability: {denied:?}"
    );
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        bundle_name,
        "denied update must leave the record untouched"
    );
}

#[test]
fn change_scope_unknown_peer_reports_behind_authorization() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_unknown";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let unknown = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        "ghost@RELAY",
        Some("*"),
    );
    assert_eq!(unknown["response"]["kind"], "error");
    assert_eq!(
        unknown["response"]["error"]["code"], "validation_unknown_principal",
        "authorized caller sees unknown-principal: {unknown:?}"
    );

    let non_relay = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        &format!("alpha@{bundle_name}"),
        Some("*"),
    );
    assert_eq!(non_relay["response"]["kind"], "error");
    assert_eq!(
        non_relay["response"]["error"]["code"], "validation_invalid_principal_id",
        "non-relay target is rejected: {non_relay:?}"
    );
}

#[test]
fn change_scope_omitted_scope_is_invalid_not_a_clear() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_omitted";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "omitted@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );

    let rejected = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        None,
    );
    assert_eq!(rejected["response"]["kind"], "error");
    assert_eq!(
        rejected["response"]["error"]["code"], "validation_invalid_arguments",
        "omitted scope must not clear: {rejected:?}"
    );
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        bundle_name,
        "rejected update must preserve the grant"
    );
}

#[test]
fn live_narrowing_applies_on_the_same_connection_without_reconnect() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_live";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "live@RELAY";
    let target = format!("alpha@{bundle_name}");

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
    let mut peer_request =
        |client: &mut UnixStream, reader: &mut BufReader<UnixStream>, request: Value| -> Value {
            request_id += 1;
            send_json(
                client,
                json!({
                    "frame": "request",
                    "request_id": format!("live-{request_id}"),
                    "request": request,
                }),
            );
            let mut response = read_json(reader);
            while response["frame"] != "response" {
                response = read_json(reader);
            }
            response
        };

    let first = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "send",
            "requester_session": peer_id,
            "message": "before narrowing",
            "targets": [target],
            "broadcast": false,
        }),
    );
    assert_eq!(
        first["response"]["kind"], "send",
        "wildcard send: {first:?}"
    );

    let first_raww = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "raww",
            "requester_session": peer_id,
            "target_session": target,
            "text": "before narrowing",
        }),
    );
    assert_eq!(
        first_raww["response"]["kind"], "raww",
        "wildcard raww: {first_raww:?}"
    );

    let narrowed = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("some-other-bundle"),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope");

    // The same connection loses ingress immediately: no reconnect, no rotation.
    let second = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "send",
            "requester_session": peer_id,
            "message": "after narrowing",
            "targets": [target],
            "broadcast": false,
        }),
    );
    assert_eq!(second["response"]["kind"], "error");
    assert_eq!(
        second["response"]["error"]["code"], "authorization_forbidden",
        "narrowed grant must deny on the existing connection: {second:?}"
    );

    let second_raww = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "raww",
            "requester_session": peer_id,
            "target_session": target,
            "text": "after narrowing",
        }),
    );
    assert_eq!(second_raww["response"]["kind"], "error");
    assert_eq!(
        second_raww["response"]["error"]["code"], "authorization_forbidden",
        "narrowed grant must deny raww: {second_raww:?}"
    );

    let principal_lookup = peer_request(
        &mut client,
        &mut reader,
        json!({"operation": "discover_principals", "namespace": bundle_name}),
    );
    assert_eq!(principal_lookup["response"]["kind"], "error");
    assert_eq!(
        principal_lookup["response"]["error"]["code"], "authorization_forbidden",
        "narrowed grant must deny principal discovery: {principal_lookup:?}"
    );

    // Namespace discovery on the same connection sees only covered candidates:
    // the narrowed scope matches nothing discoverable, so it is an empty
    // success rather than a denial.
    let namespaces = peer_request(
        &mut client,
        &mut reader,
        json!({"operation": "discover_namespaces"}),
    );
    assert_eq!(namespaces["response"]["kind"], "discover_namespaces");
    assert!(
        namespaces["response"]["namespaces"]
            .as_array()
            .expect("namespaces array")
            .is_empty(),
        "narrowed grant must hide the bundle: {namespaces:?}"
    );

    let widened = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    assert_eq!(widened["response"]["kind"], "change_scope");

    let third = peer_request(
        &mut client,
        &mut reader,
        json!({
            "operation": "send",
            "requester_session": peer_id,
            "message": "after widening",
            "targets": [target],
            "broadcast": false,
        }),
    );
    assert_eq!(
        third["response"]["kind"], "send",
        "widened grant must restore ingress on the same connection: {third:?}"
    );

    shutdown_stream(&client, "shutdown peer stream");
    join.join().expect("join peer relay thread");
}

#[test]
fn rotation_preserves_the_latest_narrowed_scope() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_rotation";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "rotated@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    let narrowed = change_scope_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope");

    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": peer_id}),
    );
    assert_eq!(rotation["response"]["kind"], "change_psk");
    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("rotated psk")
        .to_string();

    let record = store_record(&bundle_paths, peer_id);
    assert_eq!(
        record["scope"], bundle_name,
        "rotation must preserve the narrowed scope: {record}"
    );

    // The rotated credential authenticates and carries the narrowed grant.
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &rotated_psk,
        true,
    );
    assert_eq!(accepted["frame"], "hello_ack", "rotated psk: {accepted:?}");
    let denied = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &rotated_psk,
        "alpha@some-other-bundle",
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"],
        "validation_unknown_target"
    );
}

#[test]
fn wildcard_peer_reaches_bundle_targets_and_namespace_discovery() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_wildcard";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "wild@RELAY";

    let psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );

    let sent = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &psk,
        &format!("alpha@{bundle_name}"),
    );
    assert_eq!(sent["response"]["kind"], "send", "wildcard send: {sent:?}");

    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
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
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "wild-namespaces",
            "request": {"operation": "discover_namespaces"},
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["response"]["kind"], "discover_namespaces");
    let namespaces: Vec<&str> = response["response"]["namespaces"]
        .as_array()
        .expect("namespaces array")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        namespaces.contains(&bundle_name),
        "wildcard must discover the bundle: {namespaces:?}"
    );
    shutdown_stream(&client, "shutdown wildcard peer stream");
    join.join().expect("join wildcard peer thread");
}

#[test]
fn admitted_delivery_executes_after_narrowing_without_cancellation() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_uiexec";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let receiver_id = global_user_id(bundle_name);
    let admin_id = format!("admin-{receiver_id}");
    write_multi_operator_users(
        &configuration_roots,
        &[receiver_id.clone(), admin_id.clone()],
    );
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "uiexec@RELAY";

    // The receiver doubles as a live UI endpoint: its stream observes the
    // delivered envelope for sends targeting it. The admin identity performs
    // credential administration so the two concurrent Hellos never collide.
    let (mut operator_client, operator_join) =
        spawn_relay_connection(&configuration_roots, &bundle_paths);
    let operator_read = operator_client.try_clone().expect("clone operator stream");
    let mut operator_reader = BufReader::new(operator_read);
    send_json(
        &mut operator_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": receiver_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut operator_reader)["frame"], "hello_ack");

    let peer_psk = register_peer_as(
        &configuration_roots,
        &bundle_paths,
        &admin_id,
        peer_id,
        Some("*"),
    );

    // Admit a peer delivery to the operator under the wildcard grant: the
    // queued response proves admission completed.
    let (mut peer_client, peer_join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let peer_read = peer_client.try_clone().expect("clone peer stream");
    let mut peer_reader = BufReader::new(peer_read);
    send_json(
        &mut peer_client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": peer_id,
            "identity_token": peer_psk,
        }),
    );
    assert_eq!(read_json(&mut peer_reader)["frame"], "hello_ack");
    send_json(
        &mut peer_client,
        json!({
            "frame": "request",
            "request_id": "admitted-ui-send",
            "request": {
                "operation": "send",
                "requester_session": peer_id,
                "message": "admitted before narrowing",
                "targets": [receiver_id],
                "broadcast": false,
            },
        }),
    );
    let mut admitted = read_json(&mut peer_reader);
    while admitted["frame"] != "response" {
        admitted = read_json(&mut peer_reader);
    }
    assert_eq!(admitted["response"]["kind"], "send", "{admitted:?}");

    // Narrow after admission: the grant governs subsequent admissions, but
    // the admitted delivery is never retroactively cancelled.
    let narrowed = operator_request_as(
        &configuration_roots,
        &bundle_paths,
        &admin_id,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope", "{narrowed:?}");

    let event = read_until_event_type(&mut operator_reader, "incoming_message");
    assert_eq!(
        event["event"]["event_type"], "incoming_message",
        "admitted delivery must execute after narrowing: {event:?}"
    );
    shutdown_stream(&peer_client, "shutdown peer stream");
    shutdown_stream(&operator_client, "shutdown operator stream");
    peer_join.join().expect("join peer thread");
    operator_join.join().expect("join operator thread");
}

#[test]
fn scope_retry_without_grant_is_denied_not_applied() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_reauth";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "reauth@RELAY";

    register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some(bundle_name),
    );
    let narrowed = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    assert_eq!(narrowed["response"]["kind"], "change_scope");

    // Revoke the scope-update grant itself: the same replacement repeated now
    // must be denied (reauthorized under current controls), not applied.
    std::fs::write(
        configuration_roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"

[policies.controls.drop]
peer = "all"
"#,
    )
    .expect("revoke scope-update grant");
    let denied = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_scope", "principal_id": peer_id, "scope": "other"}),
    );
    assert_eq!(denied["response"]["kind"], "error");
    assert_eq!(
        denied["response"]["error"]["code"], "authorization_forbidden",
        "grantless retry must deny: {denied:?}"
    );
    assert_eq!(
        denied["response"]["error"]["details"]["capability"], "change.scope",
        "{denied:?}"
    );
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        "other",
        "denied retry must not touch the record"
    );
}

#[test]
fn committed_update_survives_response_transport_loss() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_resploss";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "resploss@RELAY";

    let peer_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("other"),
    );

    // Submit the widening update, then drop the transport without reading the
    // response: the commit does not depend on the caller observing it.
    let (mut client, join) = spawn_relay_connection(&configuration_roots, &bundle_paths);
    let read_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    let operator_id = global_user_id(bundle_name);
    send_json(
        &mut client,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator_id,
            "identity_token": "socket-trust",
        }),
    );
    assert_eq!(read_json(&mut reader)["frame"], "hello_ack");
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": "lost-response",
            "request": {"operation": "change_scope", "principal_id": peer_id, "scope": "*"},
        }),
    );
    shutdown_stream(&client, "drop response transport");
    join.join().expect("join dropped thread");

    // The replacement committed regardless: the record, a same-scope retry,
    // and live ingress all observe it.
    assert_eq!(
        store_record(&bundle_paths, peer_id)["scope"],
        "*",
        "update must commit despite response loss"
    );
    let sent = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &peer_psk,
        &format!("alpha@{bundle_name}"),
    );
    assert_eq!(sent["response"]["kind"], "send", "{sent:?}");
}
