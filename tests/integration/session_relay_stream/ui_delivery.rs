//! Connected UI stream event delivery: routing, reconnect hold, late-stream
//! registration, async-sender outcome.

use std::{
    io::BufReader,
    thread,
    time::{Duration, Instant},
};

use agentmux::{
    relay::{RelayRequest, RelayResponse, SendOutcome},
    runtime::paths::BundleRuntimePaths,
};
use serde_json::Value;
use tempfile::TempDir;
use uuid::Uuid;

use super::*;

#[test]
fn relay_send_routes_to_connected_ui_stream_with_event_frames() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let (mut ui_client, ui_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let read_stream = ui_client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);

    send_json(
        &mut ui_client,
        hello_payload(bundle_name.as_str(), &global_user_id(&bundle_name)),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-1".to_string()),
            requester_session: "alpha".to_string(),
            message: "hello ui".to_string(),
            targets: vec![global_user_id(&bundle_name)],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("send response");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Events to a relay-wide (`@GLOBAL`) UI session are addressed to its full
    // principal id; sender attribution rides in the payload's `sender_session`.
    let events = collect_events_for_target(
        &ui_client,
        &mut reader,
        &global_user_id(&bundle_name),
        None,
        Duration::from_secs(3),
    );
    let incoming_event = events
        .iter()
        .find(|value| value["event"]["event_type"] == "incoming_message")
        .expect("incoming event");
    assert_eq!(
        incoming_event["event"]["target_session"],
        global_user_id(&bundle_name)
    );
    assert_eq!(
        incoming_event["event"]["payload"]["sender_session"],
        format!("alpha@{bundle_name}")
    );

    let routed_event = events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "delivery_outcome"
                && value["event"]["payload"]["phase"] == "routed"
        })
        .expect("routed delivery outcome");
    assert!(routed_event["event"]["payload"]["outcome"].is_null());
    assert_eq!(
        routed_event["event"]["payload"]["message_id"],
        results[0].message_id
    );

    let delivered_event = events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "delivery_outcome"
                && value["event"]["payload"]["phase"] == "delivered"
        })
        .expect("delivered outcome");
    assert_eq!(delivered_event["event"]["payload"]["outcome"], "success");
    assert_eq!(
        delivered_event["event"]["payload"]["message_id"],
        results[0].message_id
    );

    ui_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown ui stream");
    ui_handle.join().expect("join relay stream");
}

#[test]
fn relay_send_waits_for_ui_reconnect_before_delivery() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");

    let (mut first_client, first_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let first_reader_stream = first_client.try_clone().expect("clone stream");
    let mut first_reader = BufReader::new(first_reader_stream);
    send_json(
        &mut first_client,
        hello_payload(bundle_name.as_str(), &global_user_id(&bundle_name)),
    );
    let _ = read_json(&mut first_reader);
    first_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown initial stream");
    first_handle.join().expect("join initial stream");

    let (mut reconnect_client, reconnect_handle) =
        spawn_relay_stream(&configuration_root, &bundle_paths);
    let reconnect_reader_stream = reconnect_client
        .try_clone()
        .expect("clone reconnect stream");
    let mut reconnect_reader = BufReader::new(reconnect_reader_stream);
    let reconnect_bundle = bundle_name.clone();
    let reconnect_thread = thread::spawn(move || {
        thread::sleep(Duration::from_millis(150));
        send_json(
            &mut reconnect_client,
            hello_payload(
                reconnect_bundle.as_str(),
                &global_user_id(&reconnect_bundle),
            ),
        );
        let ack = read_json(&mut reconnect_reader);
        let events = collect_events_for_target(
            &reconnect_client,
            &mut reconnect_reader,
            &global_user_id(&reconnect_bundle),
            None,
            Duration::from_secs(3),
        );
        reconnect_client
            .shutdown(std::net::Shutdown::Both)
            .expect("shutdown reconnect stream");
        (ack, events)
    });

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-2".to_string()),
            requester_session: "alpha".to_string(),
            message: "wait for reconnect".to_string(),
            targets: vec![global_user_id(&bundle_name)],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("send response");

    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Async dispatch returns immediately; the reconnect thread connects at
    // +150ms and still receives the terminal delivered/success event, which
    // proves the background worker held delivery until the UI reconnected.
    let (ack, events) = reconnect_thread.join().expect("join reconnect thread");
    assert_eq!(ack["frame"], "hello_ack");
    assert!(
        events
            .iter()
            .any(|value| value["event"]["event_type"] == "incoming_message")
    );
    assert!(events.iter().any(|value| {
        value["event"]["event_type"] == "delivery_outcome"
            && value["event"]["payload"]["phase"] == "routed"
    }));
    assert!(events.iter().any(|value| {
        value["event"]["event_type"] == "delivery_outcome"
            && value["event"]["payload"]["phase"] == "delivered"
            && value["event"]["payload"]["outcome"] == "success"
    }));
    reconnect_handle.join().expect("join reconnect server");
}

#[test]
fn relay_async_send_emits_terminal_delivery_outcome_to_sender_ui_stream() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");

    let (mut sender_client, sender_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let sender_read_stream = sender_client.try_clone().expect("clone sender stream");
    sender_read_stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set sender read timeout");
    let mut sender_reader = BufReader::new(sender_read_stream);
    send_json(
        &mut sender_client,
        hello_payload(bundle_name.as_str(), &global_user_id(&bundle_name)),
    );
    let sender_ack = read_json(&mut sender_reader);
    assert_eq!(sender_ack["frame"], "hello_ack");

    let response = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-async-sender".to_string()),
            requester_session: global_user_id(&bundle_name),
            message: "verify sender completion stream".to_string(),
            targets: vec![format!("alpha@{bundle_name}")],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("send response");
    let RelayResponse::Send { results, .. } = response else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);
    let expected_message_id = results[0].message_id.clone();

    let deadline = Instant::now() + Duration::from_secs(3);
    let mut observed_sender_outcome = None::<Value>;
    while Instant::now() < deadline {
        if let Some(frame) = read_json_with_timeout(&mut sender_reader)
            && frame["frame"] == "event"
            && frame["event"]["event_type"] == "delivery_outcome"
            && frame["event"]["payload"]["message_id"] == expected_message_id
        {
            let phase = frame["event"]["payload"]["phase"]
                .as_str()
                .unwrap_or_default();
            let outcome = frame["event"]["payload"]["outcome"]
                .as_str()
                .unwrap_or_default();
            if (phase == "delivered" && outcome == "success")
                || (phase == "failed" && (outcome == "timeout" || outcome == "failed"))
            {
                observed_sender_outcome = Some(frame);
                break;
            }
        }
    }
    assert!(
        observed_sender_outcome.is_some(),
        "expected sender stream to receive terminal delivery_outcome for queued async message"
    );

    sender_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown sender stream");
    sender_handle.join().expect("join sender relay stream");
}

// Regression: a configured UI target whose first send precedes its UI stream
// registration must recover and deliver via `UiTransport` once the stream
// connects. The per-target worker resolves UI routing per delivery (not latched
// for its lifetime), so the second send routes to UI even though the first bound
// the worker before any UI stream existed.
#[test]
fn relay_configured_ui_target_recovers_after_late_stream_registration() {
    let temporary = TempDir::new().expect("temporary directory");
    let bundle_name = format!("party-{}", Uuid::new_v4().simple());
    let configuration_root = write_bundle_configuration_with_ui_member(&temporary, &bundle_name);
    let state_root = temporary.path().join("state");
    let bundle_paths =
        BundleRuntimePaths::resolve(&state_root, bundle_name.as_str()).expect("bundle paths");
    let display_target = format!("display@{bundle_name}");

    // Send #1 to the configured UI target BEFORE any UI stream registers. The
    // per-target worker is created and routes this delivery on the non-UI path
    // (the registry has no UI entry yet). Under a latched resolution this would
    // bind the worker to non-UI for its lifetime.
    let first = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-ui-pre".to_string()),
            requester_session: "alpha".to_string(),
            message: "before registration".to_string(),
            targets: vec![display_target.clone()],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("first send response");
    let RelayResponse::Send { results, .. } = first else {
        panic!("expected send response");
    };
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, SendOutcome::Queued);

    // Let the worker process send #1 so the routing decision is exercised once
    // while no UI stream exists.
    thread::sleep(Duration::from_millis(300));

    // The UI client now connects and registers a Ui stream for `display`.
    let (mut ui_client, ui_handle) = spawn_relay_stream(&configuration_root, &bundle_paths);
    let read_stream = ui_client.try_clone().expect("clone stream");
    let mut reader = BufReader::new(read_stream);
    send_json(
        &mut ui_client,
        hello_payload(bundle_name.as_str(), "display"),
    );
    let hello_ack = read_json(&mut reader);
    assert_eq!(hello_ack["frame"], "hello_ack");

    // Send #2 to the same target: the worker must re-evaluate routing, observe
    // the now-registered UI stream, and deliver via UiTransport.
    let second = dispatch_request(
        RelayRequest::Send {
            request_id: Some("req-ui-post".to_string()),
            requester_session: "alpha".to_string(),
            message: "after registration".to_string(),
            targets: vec![display_target.clone()],
            broadcast: false,
            quiet_window_ms: None,
            on_behalf_of: None,
        },
        &configuration_root,
        bundle_name.as_str(),
        &bundle_paths.runtime_directory,
    )
    .expect("second send response");
    let RelayResponse::Send {
        results: second_results,
        ..
    } = second
    else {
        panic!("expected send response");
    };
    assert_eq!(second_results.len(), 1);
    assert_eq!(second_results[0].outcome, SendOutcome::Queued);

    // Collect until the SECOND message's terminal outcome. The first message was
    // held before any UI stream existed; it flushes (with its own `delivered`
    // outcome) the moment the stream registers, ahead of the second send's events.
    // Keying the terminal on the second message id keeps collection going past the
    // held message so the "after registration" `incoming_message` is observed.
    let events = collect_events_for_target(
        &ui_client,
        &mut reader,
        display_target.as_str(),
        Some(second_results[0].message_id.as_str()),
        Duration::from_secs(3),
    );
    events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "incoming_message"
                && value["event"]["payload"]["body"] == "after registration"
        })
        .expect("'after registration' incoming_message after late UI stream registration");
    let delivered = events
        .iter()
        .find(|value| {
            value["event"]["event_type"] == "delivery_outcome"
                && value["event"]["payload"]["phase"] == "delivered"
                && value["event"]["payload"]["message_id"] == second_results[0].message_id.as_str()
        })
        .expect("delivered outcome for second send after late registration");
    assert_eq!(delivered["event"]["payload"]["outcome"], "success");
    assert_eq!(
        delivered["event"]["payload"]["message_id"],
        second_results[0].message_id
    );

    ui_client
        .shutdown(std::net::Shutdown::Both)
        .expect("shutdown ui stream");
    ui_handle.join().expect("join relay stream");
}
