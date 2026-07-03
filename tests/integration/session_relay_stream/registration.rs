//! Configured bundle member hello/registration: a `policy = "operator"`
//! session can hello a fresh relay stream and the relay acknowledges the
//! qualified principal.

use std::io::BufReader;

use agentmux::runtime::paths::BundleRuntimePaths;
use tempfile::TempDir;
use uuid::Uuid;

use super::*;

#[test]
fn relay_accepts_hello_for_configured_bundle_member() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_operator_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let (mut client, handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let reader_stream = client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(reader_stream);

    send_json(&mut client, hello_payload(bundle_name.as_str(), "alpha"));
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");
    assert_eq!(hello_ack["principal_id"], format!("alpha@{bundle_name}"));

    client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown stream");
    handle.join().expect("join relay stream");
}
