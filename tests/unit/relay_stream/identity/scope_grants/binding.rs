use super::*;

#[test]
fn drop_revokes_the_peer_connection_and_isolates_re_registration() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_dropbind";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "stale@RELAY";
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

    // Dropping the peer revokes its live connection: the relay delivers a
    // typed revocation frame rather than leaving a stale session behind.
    let dropped = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "drop_peer", "principal_id": peer_id}),
    );
    assert_eq!(dropped["response"]["kind"], "drop_peer");

    let mut revoked = read_json(&mut reader);
    while revoked["frame"] != "response" {
        revoked = read_json(&mut reader);
    }
    assert_eq!(
        revoked["response"]["error"]["code"], "runtime_identity_revoked",
        "drop must revoke the peer session: {revoked:?}"
    );
    join.join().expect("join revoked peer thread");

    // Re-registering the same id mints an unrelated credential: the old PSK
    // authenticates nothing, so no stale binding can survive under the id.
    let fresh_psk = register_peer(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        peer_id,
        Some("*"),
    );
    assert_ne!(fresh_psk, psk);
    let stale_hello = hello_first_frame(&configuration_roots, &bundle_paths, peer_id, &psk, true);
    assert_eq!(stale_hello["response"]["kind"], "error");
    assert_eq!(
        stale_hello["response"]["error"]["code"], "validation_unrecognized_credential",
        "superseded credential must not authenticate: {stale_hello:?}"
    );

    let fresh = peer_send_response(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &fresh_psk,
        &target,
    );
    assert_eq!(
        fresh["response"]["kind"], "send",
        "fresh credential: {fresh:?}"
    );
}

#[test]
fn rotation_revokes_a_live_peer_session_with_typed_frame() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "ident_scope_rotrevoke";
    let configuration_roots = write_scope_configuration(&temporary, bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths = BundleRuntimePaths::resolve(&state_root, bundle_name).expect("bundle paths");
    let peer_id = "rotrevoke@RELAY";

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

    // Rotating from the operator revokes the peer's live session: a stale
    // dispatched credential cannot linger after its record is superseded.
    let rotation = operator_request(
        &configuration_roots,
        &bundle_paths,
        bundle_name,
        json!({"operation": "change_psk", "principal_id": peer_id}),
    );
    assert_eq!(rotation["response"]["kind"], "change_psk");
    let mut revoked = read_json(&mut reader);
    while revoked["frame"] != "response" {
        revoked = read_json(&mut reader);
    }
    assert_eq!(
        revoked["response"]["error"]["code"], "runtime_identity_revoked",
        "rotation must revoke the peer session: {revoked:?}"
    );
    join.join().expect("join revoked peer thread");

    let rotated_psk = rotation["response"]["psk"]
        .as_str()
        .expect("rotated psk")
        .to_string();
    let stale = hello_first_frame(&configuration_roots, &bundle_paths, peer_id, &psk, true);
    assert_eq!(stale["response"]["kind"], "error");
    assert_eq!(
        stale["response"]["error"]["code"], "validation_unrecognized_credential",
        "superseded credential must not authenticate: {stale:?}"
    );
    let accepted = hello_first_frame(
        &configuration_roots,
        &bundle_paths,
        peer_id,
        &rotated_psk,
        true,
    );
    assert_eq!(accepted["frame"], "hello_ack", "rotated psk: {accepted:?}");
}
