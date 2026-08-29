//! What reaches the pane: the paste-buffer command sequence, the envelope
//! metadata recorded alongside it, canonical addressing across bundles, and
//! the target admission refuses before any of that runs.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use agentmux::relay::{RelayRequest, RelayResponse, SendOutcome, request_relay};
use tempfile::TempDir;
use tokio::time::sleep;

use crate::support::relay_delivery::{
    spawn_relay_with_fake_tmux, spawn_relay_with_fake_tmux_and_env, wait_for_relay_ready,
    write_bundle_configuration, write_bundle_with_pubsub_member, write_fake_tmux_script,
};

use super::*;

/// A send to a configured `pubsub` member is refused synchronously at admission,
/// against a live relay: the request itself fails, nothing is queued, and no
/// terminal outcome is produced because nothing was accepted. It must also NOT
/// fall through to tmux delivery — regressing the construct-from-`session_type()`
/// model against the prior registry-routing default that misrouted non-UI targets
/// to tmux.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_send_to_configured_pubsub_member_is_refused_at_admission_and_skips_tmux() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root =
        write_bundle_with_pubsub_member(temporary.path(), bundle_name, "alpha", "pub1");
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
    );
    wait_for_relay_ready(&relay_socket).await;

    let send_response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-pubsub".to_string()),
            requester_session: "alpha".to_string(),
            message: "to a pubsub member".to_string(),
            targets: vec!["pub1@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
    )
    .expect("relay answers the send request");
    let RelayResponse::Error { error } = send_response else {
        panic!("a pubsub target should be refused at admission, not answered with a send response");
    };
    assert_eq!(error.code, "runtime_session_type_not_implemented");

    // Wait for this request's own trace before asserting what is absent from it.
    // `relay.send.request` is emitted before authorization and admission, so its
    // presence means the log holds the refused request rather than nothing at all
    // — which is what would make the absence assertion below vacuous.
    let inscriptions_path = inscriptions_root.join("relay.log");
    let deadline = Instant::now() + Duration::from_secs(5);
    let inscriptions = loop {
        let current = fs::read_to_string(&inscriptions_path).unwrap_or_default();
        if current.contains("\"event\":\"relay.send.request\"") {
            break current;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for the send request inscription, inscriptions={current:?}");
        }
        sleep(Duration::from_millis(50)).await;
    };

    shutdown_relay_gracefully(&mut child).await;

    // Nothing was accepted for pub1, so it has no queue entry and no terminal
    // outcome: no work is authorized merely to discover the stub.
    assert!(
        !inscriptions.contains("\"target_session\":\"pub1\""),
        "a refused pubsub target should produce no queue entry or outcome, inscriptions={inscriptions:?}"
    );
    let tmux_log = fs::read_to_string(&log_file).unwrap_or_default();
    assert!(
        !tmux_log.contains("pub1"),
        "pubsub target must not attempt tmux delivery, tmux_log={tmux_log:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_delivery_sends_submit_in_separate_tmux_command() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux_and_env(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
        &[("FAKE_TMUX_CAPTURE_MODE", "stable")],
    );
    wait_for_relay_ready(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-submit-separate-enter".to_string()),
            requester_session: "alpha".to_string(),
            message: "A".repeat(6_000),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
    )
    .expect("send request should succeed");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Async delivery runs in a background worker; wait for the fake tmux log
    // to record the unbracketed carriage-return paste (the submit) before
    // reaping the relay so the full paste-buffer command sequence is
    // observable.
    let delivery_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if fs::read_to_string(&log_file)
            .map(|log| log.lines().any(is_submit_paste_line))
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < delivery_deadline,
            "async delivery did not complete within timeout"
        );
        sleep(Duration::from_millis(20)).await;
    }

    shutdown_relay_gracefully(&mut child).await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    let log_lines: Vec<&str> = log.lines().collect();
    // The body is delivered as a single bracketed paste (`-p`); the submit
    // is a separate unbracketed paste carrying a bare carriage return. Both
    // target the pane; only the body count is asserted as exactly-one here
    // (a chunked large payload would show more than one bracketed paste).
    let body_indexes: Vec<usize> = log_lines
        .iter()
        .enumerate()
        .filter(|(_, line)| is_body_paste_line(line))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        body_indexes.len(),
        1,
        "expected exactly one bracketed body paste for large payload, log={log:?}"
    );
    let buffer_content = read_paste_buffer_content(&log_file, log_lines[body_indexes[0]]);
    assert!(
        buffer_content.contains("Message-Id:"),
        "expected pane envelope to include Message-Id header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.contains("Date:"),
        "expected pane envelope to include Date header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.contains("From:"),
        "expected pane envelope to include From header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.contains("To:"),
        "expected pane envelope to include To header, content={buffer_content:?}"
    );
    assert!(
        buffer_content.starts_with("--agentmux-"),
        "expected paste buffer to begin with leading boundary fence, content={buffer_content:?}"
    );
    assert!(
        !buffer_content.contains("Envelope-Version:"),
        "pane envelope must omit Envelope-Version header, content={buffer_content:?}"
    );
    assert!(
        !buffer_content.contains("multipart/mixed; boundary="),
        "pane envelope must omit top-level multipart header, content={buffer_content:?}"
    );
    assert!(
        !buffer_content.contains("Content-Transfer-Encoding:"),
        "pane envelope must omit per-part transfer encoding header, content={buffer_content:?}"
    );
    let submit_index = log_lines
        .iter()
        .position(|line| is_submit_paste_line(line))
        .expect("expected separate unbracketed carriage-return paste (the submit)");
    assert!(
        body_indexes[0] < submit_index,
        "expected body paste before submit paste, log={log:?}"
    );
    assert_eq!(
        read_paste_buffer_content(&log_file, log_lines[submit_index]),
        "\r",
        "submit paste must carry a bare carriage return"
    );
    assert!(
        !log.contains("send-keys"),
        "submit must go through paste-buffer, not send-keys, log={log:?}"
    );

    let inscriptions =
        fs::read_to_string(inscriptions_root.join("relay.log")).expect("read relay inscriptions");
    // Isolate the envelope-metadata event itself: a whole-log substring scan
    // would match `bundle_name`/`namespace` carried by unrelated inscriptions
    // (e.g. relay.send.async.queued), so assert on this event's own `details`.
    let metadata_details = inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| {
            event.get("event").and_then(serde_json::Value::as_str)
                == Some("relay.send.envelope.metadata")
        })
        .and_then(|event| event.get("details").cloned())
        .expect("expected a relay.send.envelope.metadata inscription with details");
    let details = metadata_details
        .as_object()
        .expect("envelope metadata details is an object");
    // Values, not field names. A presence check passes against an event whose
    // every field is empty or belongs to some other send, which is exactly the
    // correlation this inscription exists to provide: it is what lets a reader
    // tie a pane envelope back to the request that produced it.
    assert_eq!(
        details.get("namespace").and_then(serde_json::Value::as_str),
        Some(bundle_name),
        "envelope metadata must name this send's namespace: {details:?}"
    );
    assert_eq!(
        details
            .get("sender_session")
            .and_then(serde_json::Value::as_str),
        Some("alpha@party"),
        "envelope metadata must name the canonical sender: {details:?}"
    );
    assert_eq!(
        details
            .get("target_sessions")
            .and_then(serde_json::Value::as_array)
            .map(|targets| targets
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect::<Vec<_>>()),
        Some(vec!["alpha@party"]),
        "envelope metadata must name the canonical targets: {details:?}"
    );
    // Correlated against the response rather than merely nonempty: a populated
    // id from an unrelated send would satisfy nonemptiness and defeat the whole
    // purpose of carrying one.
    assert_eq!(
        details
            .get("message_id")
            .and_then(serde_json::Value::as_str),
        Some(results[0].message_id.as_str()),
        "envelope metadata must carry the message id the send returned: {details:?}"
    );
    assert_eq!(
        details
            .get("schema_version")
            .and_then(serde_json::Value::as_str),
        Some("1"),
        "envelope metadata must name its schema version: {details:?}"
    );
    assert!(
        details
            .get("created_at")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|created| created.contains('T') && created.ends_with('Z')),
        "envelope metadata must carry a UTC timestamp: {details:?}"
    );
    assert!(
        !details.contains_key("bundle_name"),
        "envelope metadata must use namespace, not the retired bundle_name field, \
         details={details:?}"
    );
}

/// A pane reporting `#{pane_in_mode} = 1` (tmux copy-mode, as a
/// mouse-wheel scroll leaves it) must NOT block delivery: the classifier
/// ignores copy-mode, so the message is both pasted and submitted. This
/// asserts the command shape our code emits under copy-mode — the fake
/// tmux cannot model paste-through-copy-mode semantics, only the command
/// sequence; the real-tmux behavior is covered separately in
/// relay_delivery_async.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_async_delivery_injects_even_while_pane_in_mode() {
    let temporary = TempDir::new().expect("temporary");
    let bundle_name = "party";
    let config_root = write_bundle_configuration(temporary.path(), bundle_name, &["alpha"]);
    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux_and_env(
        bundle_name,
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
        &[
            ("FAKE_TMUX_CAPTURE_MODE", "stable"),
            ("FAKE_TMUX_PANE_IN_MODE", "1"),
        ],
    );
    wait_for_relay_ready(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-interaction-mode".to_string()),
            requester_session: "alpha".to_string(),
            message: "interaction marker".to_string(),
            targets: vec!["alpha@party".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
    )
    .expect("send request should complete");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Delivery must proceed to the submit despite pane_in_mode=1. Wait for
    // the unbracketed carriage-return paste; absent the gate removal this
    // would never appear.
    let delivery_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if fs::read_to_string(&log_file)
            .map(|log| log.lines().any(is_submit_paste_line))
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < delivery_deadline,
            "delivery did not reach submit while pane_in_mode active"
        );
        sleep(Duration::from_millis(20)).await;
    }

    shutdown_relay_gracefully(&mut child).await;

    let log = fs::read_to_string(&log_file).expect("read fake tmux log");
    assert!(
        log.lines().any(is_body_paste_line),
        "body must be pasted even while pane_in_mode active, log={log:?}"
    );
    assert!(
        !log.contains("send-keys"),
        "delivery must go through paste-buffer, not send-keys, log={log:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_async_delivery_envelope_addresses_carry_canonical_ids_across_bundles() {
    let temporary = TempDir::new().expect("temporary");
    let config_root = write_bundle_configuration(temporary.path(), "party", &["alpha", "bravo"]);
    // A second namespace plus a send scope that reaches it: the fixture's
    // policies file caps send at home.
    fs::write(
        config_root.base_layer().join("bundles").join("qa.toml"),
        r#"format-version = 1
autostart = true

[[sessions]]
id = "zulu"
name = "zulu"
directory = "/tmp"
coder = "default"
"#,
    )
    .expect("write qa bundle config");
    fs::write(
        config_root.base_layer().join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
find = "self"
list = "home"
look = "self"
send = "all"
"#,
    )
    .expect("widen send scope for cross-bundle delivery");

    let state_root = temporary.path().join("state");
    let fake_tmux_script = temporary.path().join("fake-tmux.sh");
    let attempts_file = temporary.path().join("attempts.txt");
    let log_file = temporary.path().join("fake-tmux.log");
    let inscriptions_root = temporary.path().join("inscriptions");
    write_fake_tmux_script(&fake_tmux_script, &attempts_file, &log_file);

    let relay_socket = state_root.join("relay.sock");
    let mut child = spawn_relay_with_fake_tmux_and_env(
        "party",
        &config_root,
        &state_root,
        &inscriptions_root,
        &fake_tmux_script,
        &[("FAKE_TMUX_CAPTURE_MODE", "stable")],
    );
    wait_for_relay_ready(&relay_socket).await;

    let response = request_relay(
        &relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some("req-canonical-addresses".to_string()),
            requester_session: "alpha".to_string(),
            message: "cross-bundle co-recipient visibility".to_string(),
            targets: vec!["bravo@party".to_string(), "zulu@qa".to_string()],
            broadcast: false,
            on_behalf_of: None,
        },
    )
    .expect("cross-bundle send request should succeed");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 2);
    assert!(
        results
            .iter()
            .all(|result| result.outcome == SendOutcome::Queued),
        "expected both targets queued, results={results:?}"
    );

    // Async delivery pastes one buffer per target; wait until both envelopes
    // land, then identify each by its To header.
    let delivery_deadline = Instant::now() + Duration::from_secs(5);
    let (bravo_envelope, zulu_envelope) = loop {
        let envelopes = read_all_paste_buffers(temporary.path());
        let bravo = envelopes
            .iter()
            .find(|content| content.contains("To: bravo <session:bravo@party>"))
            .cloned();
        let zulu = envelopes
            .iter()
            .find(|content| content.contains("To: zulu <session:zulu@qa>"))
            .cloned();
        if let (Some(bravo), Some(zulu)) = (bravo, zulu) {
            break (bravo, zulu);
        }
        assert!(
            Instant::now() < delivery_deadline,
            "async deliveries did not complete, envelopes={envelopes:?}"
        );
        sleep(Duration::from_millis(20)).await;
    };

    shutdown_relay_gracefully(&mut child).await;

    assert!(
        bravo_envelope.contains("From: alpha <session:alpha@party>"),
        "expected canonical sender address, envelope={bravo_envelope:?}"
    );
    // The cross-bundle co-recipient is absent from the delivery bundle's
    // configuration, so its Cc entry carries the canonical id alone.
    assert!(
        bravo_envelope.contains("Cc: zulu@qa <session:zulu@qa>"),
        "expected cross-bundle co-recipient in Cc, envelope={bravo_envelope:?}"
    );
    assert!(
        zulu_envelope.contains("From: alpha <session:alpha@party>"),
        "expected canonical sender address, envelope={zulu_envelope:?}"
    );
    assert!(
        zulu_envelope.contains("Cc: bravo@party <session:bravo@party>"),
        "expected cross-bundle co-recipient in Cc, envelope={zulu_envelope:?}"
    );
}

fn read_all_paste_buffers(directory: &Path) -> Vec<String> {
    let mut contents = Vec::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return contents;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        if file_name
            .to_string_lossy()
            .starts_with("fake-tmux.log.buffer.")
            && let Ok(content) = fs::read_to_string(entry.path())
        {
            contents.push(content);
        }
    }
    contents
}
