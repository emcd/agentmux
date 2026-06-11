use super::*;

#[test]
fn permission_decision_rejects_submitter_without_grant_capability() {
    // Permission decisioning is now gated on the `grant` policy capability
    // rather than a hello-asserted client class. The `alpha` bundle member
    // resolves to the default policy, which omits `grant`.
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_non_grant";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": format!("alpha@{bundle_name}"),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled"
            }
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "grant"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_payload_actor_spoof_field() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_actor_spoof";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled",
                "ui_session_id": "spoofed"
            }
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_denial_uses_grant_capability() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_grant_capability";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled"
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );
    assert_eq!(
        response["response"]["error"]["details"]["capability"],
        "grant"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_empty_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_empty_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "selected",
                "option_id": "   "
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_selected_without_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_selected_missing_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "selected"
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_decision_rejects_cancelled_with_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_cancelled_with_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-1",
                "outcome": "cancelled",
                "option_id": "allow-once"
            }
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_snapshot_then_replay_carries_option_metadata() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_snapshot_options";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_grant(&configuration_root, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue_with_options(
        &bundle_paths.runtime_directory,
        &[("perm-aaa", "allow-once"), ("perm-bbb", "allow-once")],
    );

    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let snapshot = read_until_event_type(&mut reader, "permission.snapshot");
    let snapshot_payload = &snapshot["event"]["payload"];
    assert_eq!(snapshot_payload["pending_count"], 2);
    assert_eq!(
        snapshot_payload["permission_request_ids"],
        json!(["perm-aaa", "perm-bbb"])
    );

    let first = read_until_event_type(&mut reader, "permission.requested");
    let first_payload = &first["event"]["payload"];
    assert_eq!(first_payload["permission_request_id"], "perm-aaa");
    assert_eq!(
        first_payload["target_session"],
        format!("alpha@{bundle_name}")
    );
    let options = first_payload["requested_details"]["options"]
        .as_array()
        .expect("options array on permission.requested payload");
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["option_id"], "allow-once");
    assert_eq!(options[0]["name"], "Allow once");
    assert_eq!(options[0]["kind"], "allow_once");
    assert_eq!(options[1]["option_id"], "allow-once-reject");
    assert_eq!(options[1]["kind"], "reject_once");

    let second = read_until_event_type(&mut reader, "permission.requested");
    assert_eq!(
        second["event"]["payload"]["permission_request_id"],
        "perm-bbb"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_request_persists_across_authorized_ui_reconnect() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_persists";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_grant(&configuration_root, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-persistent",
        "allow-once",
    );

    let hello_frame = json!({
        "frame": "hello",
        "schema_version": "1",
        "principal_id": global_user_id(bundle_name),
        "identity_token": "socket-trust",
    });

    let (mut first_client, first_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let first_read = first_client.try_clone().expect("clone first stream");
    let mut first_reader = BufReader::new(first_read);
    send_json(&mut first_client, hello_frame.clone());
    let ack = read_json(&mut first_reader);
    assert_eq!(ack["frame"], "hello_ack");
    let snapshot = read_until_event_type(&mut first_reader, "permission.snapshot");
    assert_eq!(snapshot["event"]["payload"]["pending_count"], 1);
    let requested = read_until_event_type(&mut first_reader, "permission.requested");
    assert_eq!(
        requested["event"]["payload"]["permission_request_id"],
        "perm-persistent"
    );
    shutdown_stream(&first_client, "shutdown first client");
    first_handle.join().expect("join first relay thread");

    thread::sleep(std::time::Duration::from_millis(200));

    let (mut second_client, second_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let second_read = second_client.try_clone().expect("clone second stream");
    let mut second_reader = BufReader::new(second_read);
    send_json(&mut second_client, hello_frame);
    let ack = read_json(&mut second_reader);
    assert_eq!(ack["frame"], "hello_ack");
    let snapshot = read_until_event_type(&mut second_reader, "permission.snapshot");
    assert_eq!(snapshot["event"]["payload"]["pending_count"], 1);
    let requested = read_until_event_type(&mut second_reader, "permission.requested");
    assert_eq!(
        requested["event"]["payload"]["permission_request_id"],
        "perm-persistent"
    );

    shutdown_stream(&second_client, "shutdown second client");
    second_handle.join().expect("join second relay thread");
}

#[test]
fn permission_resolve_selected_emits_resolved_event_with_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_selected_emit";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_grant(&configuration_root, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-selected",
        "allow-once",
    );

    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let ack = read_json(&mut reader);
    assert_eq!(ack["frame"], "hello_ack");
    let _snapshot = read_until_event_type(&mut reader, "permission.snapshot");
    let _requested = read_until_event_type(&mut reader, "permission.requested");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-resolve-selected",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-selected",
                "outcome": "selected",
                "option_id": "allow-once"
            }
        }),
    );

    let resolved = read_until_event_type(&mut reader, "permission.resolved");
    let payload = &resolved["event"]["payload"];
    assert_eq!(payload["permission_request_id"], "perm-selected");
    assert_eq!(payload["outcome"], "selected");
    assert_eq!(payload["reason_code"], Value::Null);
    assert_eq!(payload["decided_by"], global_user_id(bundle_name));

    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-resolve-selected");
    assert_eq!(response["response"]["kind"], "permission_decision");
    assert_eq!(response["response"]["outcome"], "selected");
    assert_eq!(response["response"]["status"], "resolved");
    assert_eq!(
        response["response"]["permission_request_id"],
        "perm-selected"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_resolve_cancelled_emits_resolved_event_with_reason_code() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_cancelled_emit";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_grant(&configuration_root, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-cancelled",
        "allow-once",
    );

    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let ack = read_json(&mut reader);
    assert_eq!(ack["frame"], "hello_ack");
    let _snapshot = read_until_event_type(&mut reader, "permission.snapshot");
    let _requested = read_until_event_type(&mut reader, "permission.requested");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-resolve-cancelled",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-cancelled",
                "outcome": "cancelled"
            }
        }),
    );

    let resolved = read_until_event_type(&mut reader, "permission.resolved");
    let payload = &resolved["event"]["payload"];
    assert_eq!(payload["permission_request_id"], "perm-cancelled");
    assert_eq!(payload["outcome"], "cancelled");
    assert_eq!(
        payload["reason_code"],
        "runtime_permission_request_cancelled"
    );
    assert_eq!(payload["decided_by"], global_user_id(bundle_name));

    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-resolve-cancelled");
    assert_eq!(response["response"]["kind"], "permission_decision");
    assert_eq!(response["response"]["outcome"], "cancelled");
    assert_eq!(
        response["response"]["reason_code"],
        "runtime_permission_request_cancelled"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_max_pending_out_of_range_is_rejected() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_max_pending_invalid";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_grant(&configuration_root, "home");
    std::fs::write(
        configuration_root.join("relay.toml"),
        r#"
[relay.permission]
max-pending = 10000
"#,
    )
    .expect("write relay configuration");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");

    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let ack = read_json(&mut reader);
    assert_eq!(ack["frame"], "hello_ack");
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_arguments"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "relay.permission.max-pending"
    );
    assert_eq!(response["response"]["error"]["details"]["value"], 10000);
    assert_eq!(response["response"]["error"]["details"]["maximum"], 4096);

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}

#[test]
fn permission_resolve_selected_rejects_unknown_option_id() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party_permission_unknown_option";
    let configuration_root = write_bundle_configuration(&temporary, bundle_name);
    write_tui_configuration(&configuration_root, "default", bundle_name);
    write_policies_with_grant(&configuration_root, "home");
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    seed_permission_queue(
        &bundle_paths.runtime_directory,
        "perm-unknown-option",
        "allow-once",
    );
    let (mut client_stream, join_handle) =
        spawn_relay_connection(&configuration_root, &bundle_paths);
    let read_stream = client_stream.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut client_stream,
        json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": global_user_id(bundle_name),
            "identity_token": "socket-trust",
        }),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    send_json(
        &mut client_stream,
        json!({
            "frame": "request",
            "namespace": bundle_name,
            "request_id": "req-1",
            "request": {
                "operation": "permission_resolve",
                "permission_request_id": "perm-unknown-option",
                "outcome": "selected",
                "option_id": "not-present"
            }
        }),
    );
    let mut response = read_json(&mut reader);
    while response["frame"] != "response" {
        response = read_json(&mut reader);
    }
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], "req-1");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "validation_invalid_params"
    );
    assert_eq!(
        response["response"]["error"]["details"]["field"],
        "option_id"
    );
    assert_eq!(
        response["response"]["error"]["details"]["value"],
        "not-present"
    );

    shutdown_stream(&client_stream, "shutdown client stream");
    join_handle.join().expect("join relay thread");
}
