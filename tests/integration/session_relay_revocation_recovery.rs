//! Recovery of a live, store-backed session whose credential the relay rotates
//! out from under it.
//!
//! The unit tests in `relay_stream_client` pin the two halves of the client's
//! reconnect logic against a hand-built server. This exercises the same path
//! against a real relay process performing a real revocation, and covers what a
//! socket seam cannot: that the redial re-reads the credential from disk, so a
//! session recovers only once the rotated secret is actually in place.
//!
//! The requester must hold a real credential. A socket-trust Hello records no
//! `authenticated_identity` and is never matched by the revocation sweep, so a
//! test written against the harness default cannot reach this path at all and
//! would pass against unfixed code.

use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::{Duration, Instant},
};

use agentmux::{
    configuration::ConfigurationRoots,
    relay::{RelayRequest, RelayResponse, RelayStreamSession},
    runtime::paths::session_identity_psk_path,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::support::relay_delivery::{
    spawn_relay_with_fake_tmux, wait_for_relay_ready, write_bundle_configuration,
    write_fake_tmux_script,
};

/// Bounds every wait in this test, so a regression that never produces the
/// awaited state fails rather than parks.
const RECOVERY_BUDGET: Duration = Duration::from_secs(5);

/// Grants credential administration to the operator principal, and binds the
/// relay-wide user session to it.
fn write_credential_admin_configuration(configuration_roots: &ConfigurationRoots, operator: &str) {
    fs::write(
        configuration_roots.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "home"

[[policies]]
id = "operator"

[policies.controls]
find = "self"
list = "home"
look = "home"
send = "home"

[policies.controls.new]
peer = "all"

[policies.controls.change]
psk = "all"
"#,
    )
    .expect("write policies configuration");
    fs::write(
        configuration_roots.base_layer().join("users.toml"),
        format!(
            r#"
default-session = "{operator}"

[[sessions]]
id = "{operator}"
policy = "operator"

[sessions.ui]
"#
        ),
    )
    .expect("write users configuration");
}

/// Issues one credential-administration request as the operator over a
/// throwaway connection, and returns the response frame's `response` object.
fn operator_request(socket_path: &Path, operator: &str, request: Value) -> Value {
    let mut client = UnixStream::connect(socket_path).expect("connect operator stream");
    client
        .set_read_timeout(Some(RECOVERY_BUDGET))
        .expect("bound operator reads");
    let mut reader = BufReader::new(client.try_clone().expect("clone operator stream"));
    write_frame(
        &mut client,
        &json!({
            "frame": "hello",
            "schema_version": "1",
            "principal_id": operator,
            "identity_token": "socket-trust",
        }),
    );
    let ack = read_frame(&mut reader);
    assert_eq!(
        ack["frame"], "hello_ack",
        "operator hello not acked: {ack:?}"
    );
    write_frame(
        &mut client,
        &json!({
            "frame": "request",
            "request_id": "admin-1",
            "request": request,
        }),
    );
    let mut frame = read_frame(&mut reader);
    while frame["frame"] != "response" {
        frame = read_frame(&mut reader);
    }
    frame["response"].clone()
}

fn write_frame(stream: &mut UnixStream, payload: &Value) {
    let text = serde_json::to_string(payload).expect("encode frame");
    stream
        .write_all(format!("{text}\n").as_bytes())
        .expect("write frame");
    stream.flush().expect("flush frame");
}

fn read_frame(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .unwrap_or_else(|source| panic!("no frame within {RECOVERY_BUDGET:?}: {source}"));
    serde_json::from_str(line.trim_end()).expect("decode frame")
}

fn list_request() -> RelayRequest {
    RelayRequest::List {
        requester_session: Some("alpha".to_string()),
    }
}

fn assert_list_response(response: &RelayResponse, context: &str) {
    match response {
        RelayResponse::List { .. } => {}
        other => panic!("{context}: unexpected response {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_revoked_session_redials_and_recovers_once_its_credential_is_refreshed() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = "party";
    let configuration_roots = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let operator = "operator@GLOBAL";
    write_credential_admin_configuration(&configuration_roots, operator);

    let state_root = temporary.path().join("state");
    let inscriptions_root = temporary.path().join("inscriptions");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    write_fake_tmux_script(
        &fake_tmux_script,
        &temporary.path().join("attempts.txt"),
        &temporary.path().join("fake-tmux.log"),
    );
    let _relay = spawn_relay_with_fake_tmux(
        bundle_name,
        &configuration_roots,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    let relay_socket = state_root.join("relay.sock");
    wait_for_relay_ready(&relay_socket).await;

    let principal_id = format!("alpha@{bundle_name}");
    let credential_path = session_identity_psk_path(&state_root, bundle_name, "alpha");

    // Provision alpha with a real credential, written to the canonical path the
    // session reads at Hello. Without this the session is socket-trust and the
    // revocation sweep would never match it.
    let provisioned = operator_request(
        &relay_socket,
        operator,
        json!({
            "operation": "new_peer",
            "principal_id": principal_id,
            "destination": {"kind": "config"},
        }),
    );
    assert_eq!(
        provisioned["kind"], "new_peer",
        "provisioning rejected: {provisioned:?}"
    );
    assert!(
        credential_path.exists(),
        "credential destination config must write {}",
        credential_path.display()
    );

    let mut session = RelayStreamSession::new(
        relay_socket.clone(),
        bundle_name.to_string(),
        "alpha".to_string(),
    );
    let established = session
        .request(&list_request())
        .expect("store-backed session should establish");
    assert_list_response(&established, "before rotation");

    // Rotate alpha's credential, returning the new secret in the response so it
    // is NOT written to disk yet. The relay revokes alpha's live connection --
    // typed frame, then close -- leaving the session holding a dead socket with
    // readable bytes in it.
    let rotation = operator_request(
        &relay_socket,
        operator,
        json!({
            "operation": "change_psk",
            "principal_id": principal_id,
        }),
    );
    assert_eq!(
        rotation["kind"], "change_psk",
        "rotation rejected: {rotation:?}"
    );
    let rotated_psk = rotation["psk"]
        .as_str()
        .expect("rotation must return the raw psk")
        .to_string();

    // Withholding the secret makes the redial observable. Requests keep
    // succeeding on the established connection until the revocation lands, and
    // the session must then be REJECTED at Hello on its stale credential --
    // something only a connection that was actually dialled can be. A session
    // that never redials never reaches a rejection: it reports a transport
    // failure against the same dead socket forever, which is what the wedge
    // looked like in the field.
    //
    // A transport failure is tolerated on the way there rather than asserted
    // against. The liveness probe classifies the hangup before the write in
    // every ordinary interleaving, but a revocation landing between that probe
    // and the write is legitimately reported once; pinning zero failures here
    // would trade a real defect for a rare spurious one. The unit tests hold
    // each half of that behaviour to an exact accounting.
    let deadline = Instant::now() + RECOVERY_BUDGET;
    let mut last_error = None;
    loop {
        match session.request(&list_request()) {
            Ok(response) => assert_list_response(&response, "before the revocation landed"),
            Err(source) => {
                if source.to_string().contains("relay hello rejected") {
                    break;
                }
                last_error = Some(source);
            }
        }
        assert!(
            Instant::now() < deadline,
            "a revoked session must redial within {RECOVERY_BUDGET:?} and be rejected on its \
             stale credential; it never dialled. Last transport failure: {last_error:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    }

    // With the rotated secret in place the very next request must succeed: the
    // redial re-reads the credential path rather than presenting the copy it
    // first connected with.
    fs::write(&credential_path, &rotated_psk).expect("refresh credential");
    let recovered = session
        .request(&list_request())
        .expect("a refreshed credential must let the session reconnect");
    assert_list_response(&recovered, "after refreshing the credential");
}
