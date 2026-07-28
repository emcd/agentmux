//! Choose authorization: a `policy = "operator"` session may `choices_list`
//! its own pending requests; a `choose = "none"` principal cannot `choices_pick`
//! a request it did not submit.

use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use serde_json::json;
use tempfile::TempDir;
use uuid::Uuid;

use super::*;

#[test]
fn relay_choices_list_succeeds_for_grant_authorized_principal() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_operator_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    std::fs::create_dir_all(&bundle_paths.runtime_directory).expect("create runtime directory");

    let (mut client, handle) = spawn_relay_stream(&configuration_roots, &bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);

    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {"operation": "choices_list"},
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["request_id"], request_id);
    assert_eq!(response["response"]["kind"], "choices_list");
    let entries = response["response"]["pending_requests"]
        .as_array()
        .expect("pending_requests array");
    assert!(entries.is_empty(), "no requests have been queued yet");

    client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown stream");
    handle.join().expect("join relay stream");
}

#[test]
fn relay_choices_pick_rejects_submitter_without_grant() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_roots = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let (mut client, handle) = spawn_relay_stream(&configuration_roots, &bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);

    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let request_id = format!("req-{}", Uuid::new_v4().simple());
    send_json(
        &mut client,
        json!({
            "frame": "request",
            "request_id": request_id,
            "request": {
                "operation": "choices_pick",
                "choice_request_id": "perm-1",
                "outcome": "cancelled",
            },
        }),
    );
    let response = read_json(&mut reader);
    assert_eq!(response["frame"], "response");
    assert_eq!(response["response"]["kind"], "error");
    assert_eq!(
        response["response"]["error"]["code"],
        "authorization_forbidden"
    );

    client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown stream");
    handle.join().expect("join relay stream");
}
