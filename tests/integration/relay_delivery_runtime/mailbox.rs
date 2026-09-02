//! What the relay's per-target mailbox holds while the push path is still the
//! only thing delivering out of it.
//!
//! Every admitted entry now gains a relay-built payload and a position in its
//! target's mailbox, and the push path writes that stored artifact rather than
//! rendering one of its own. Nothing peeks yet, and nothing acknowledges — the
//! cursor moves only because the push path's terminal transition retires each
//! entry's position as it resolves it.
//!
//! Two things follow from that arrangement and neither is demonstrated by the
//! code that arranges it, which is why they are pinned here against a live relay
//! rather than argued from the retirement semantics: the mailbox must stay
//! bounded under sustained delivery, and what reaches the target must be the
//! artifact the mailbox holds. Either failing is a gate on the cutover, not a
//! defect to fix afterwards — an unbounded mailbox is a leak the executor would
//! inherit, and a delivered envelope that differs from the stored one would mean
//! this arrangement proves nothing about the executor's future input.

use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use agentmux::relay::{RelayRequest, RelayResponse, SendOutcome, request_relay};
use serde_json::Value;
use tempfile::TempDir;
use tokio::time::sleep;

use crate::support::relay_delivery::{
    spawn_relay_with_fake_tmux_and_env, wait_for_relay_ready, write_bundle_configuration,
    write_fake_tmux_script,
};

use super::*;

/// How many messages each test drives through one target. Enough that an
/// unbounded mailbox is visible as growth rather than as a single off-by-one,
/// and small enough that the sends stay sequential within the test's budget.
const SENDS: usize = 4;

/// The mailbox returns to empty as the push path delivers, the cursor moves with
/// it, the reservation behind it comes back, and each entry is resolved once.
///
/// The reasoning this replaces: the push path terminalizes every entry, and the
/// terminal transition retires that entry's mailbox position and releases its
/// quota, so the cursor advances and the reservation returns even though nothing
/// acknowledges. Sound, and still only reasoning — the retirement it depends on
/// was added for an acknowledgment path that has no caller yet, so nothing had
/// ever exercised it from the live delivery path.
///
/// Each enqueue reports the depth, cursor, and reservation it joined, under the
/// same lock as the insertion. Sends are awaited to completion one at a time, so
/// at each enqueue the mailbox holds exactly the entry being placed, the cursor
/// names every entry before it, and the reservation covers that entry alone. A
/// mailbox that accumulated would report a depth climbing with the send count
/// and a cursor stuck at zero; a quota that leaked would report the reservation
/// climbing with it. Depth and quota are separate state released by one
/// transition, so either can return while the other does not.
///
/// This test carries the relay's half of "no entry is delivered twice or left
/// unresolved": exactly one terminal outcome per message, and a wait that would
/// time out on an entry that was never resolved at all. The pane's half — that
/// no message reached the target twice — is
/// [`the_delivered_envelope_carries_the_stamp_the_mailbox_stored`], which is
/// where the rendered envelopes are read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mailbox_returns_to_empty_as_the_push_path_delivers() {
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

    let inscriptions_path = inscriptions_root.join("relay.log");
    let mut message_ids = Vec::with_capacity(SENDS);
    for index in 1..=SENDS {
        let message_id = send_one(&relay_socket, index).await;
        // Awaited to *completion*, not merely to the paste: the position is
        // retired by the terminal transition, which runs after the write
        // resolves. Waiting on the paste alone would let the next send be
        // admitted while this one still occupied its slot, and the depth this
        // test reads would be a report about scheduling rather than about the
        // mailbox.
        wait_for_completed_send(&inscriptions_path, message_id.as_str()).await;
        message_ids.push(message_id);
    }

    shutdown_relay_gracefully(&mut child).await;

    let inscriptions = fs::read_to_string(&inscriptions_path).expect("read relay inscriptions");
    let enqueued: Vec<Value> =
        inscription_details(&inscriptions, "relay.delivery.mailbox.enqueued")
            .into_iter()
            .filter(|details| {
                details.get("target_session").and_then(Value::as_str) == Some("alpha")
            })
            .collect();
    assert_eq!(
        enqueued.len(),
        SENDS,
        "every admitted entry fills the position admission gave it exactly once: \
         {enqueued:?}"
    );
    for (index, details) in enqueued.iter().enumerate() {
        let placed = index as u64;
        assert_eq!(
            details.get("message_id").and_then(Value::as_str),
            Some(message_ids[index].as_str()),
            "mailbox positions are filled in the order the sends were admitted: {details:?}"
        );
        assert_eq!(
            details.get("sequence").and_then(Value::as_u64),
            Some(placed + 1),
            "admission placed each entry after the one before it: {details:?}"
        );
        assert_eq!(
            details.get("mailbox_depth").and_then(Value::as_u64),
            Some(1),
            "a delivered entry leaves its position before the next one arrives: {details:?}"
        );
        assert_eq!(
            details.get("cursor").and_then(Value::as_u64),
            Some(placed),
            "the cursor advances over every retired position rather than stalling \
             behind the first: {details:?}"
        );
        assert_eq!(
            details
                .get("target_envelopes_reserved")
                .and_then(Value::as_u64),
            Some(1),
            "a delivered entry's reservation is released before the next is \
             admitted: {details:?}"
        );
        assert_eq!(
            details.get("target_bytes_reserved").and_then(Value::as_u64),
            Some(message_body(index + 1).len() as u64),
            "the released reservation returns this target's byte count to the one \
             entry it now holds: {details:?}"
        );
        // The doorbell, which is otherwise invisible from outside the relay:
        // nothing waits on it until the cutover, so a ring leaves no other
        // trace. Each of these sends arrives at a mailbox the one before it
        // emptied, which is exactly the transition a doorbell reports — so a
        // `false` here means either that the generation never registered one or
        // that the transition was misjudged, and the two failures the relay
        // could not otherwise be caught in are the whole reason this is read
        // against a live worker rather than only in the ledger's own tests.
        assert_eq!(
            details.get("doorbell_rung").and_then(Value::as_bool),
            Some(true),
            "each generation registers a doorbell and the relay rings it when \
             its target's head becomes peekable: {details:?}"
        );
    }

    // Every entry resolved, exactly once, and *delivered*. The outcome value is
    // asserted because the three figures above are all satisfied by a relay that
    // fails every delivery: a member refused before its write is admitted,
    // enqueued, and terminalized like any other, so depth, cursor, and quota all
    // return exactly as they do here. Without this the test would pass against a
    // target nothing ever reached, which is how it first passed while a probe had
    // broken delivery outright.
    //
    // The count is a tripwire rather than a demonstration, and worth labelling as
    // one. Two probes that submitted each member twice produced no second outcome:
    // the terminal transition suppresses the loser, and on the happy path there is
    // only ever one resolver, so no local edit here makes the count reach two.
    // What adjudicates the transition under genuine contention is
    // `admission::terminal`'s own inline test, which races eight resolvers at a
    // barrier. This assertion guards against a future second resolver reaching
    // the live delivery path; it does not establish that one is handled.
    let completed = inscription_details(&inscriptions, "relay.send.async.completed");
    for message_id in &message_ids {
        let owned: Vec<&Value> = completed
            .iter()
            .filter(|details| {
                details.get("message_id").and_then(Value::as_str) == Some(message_id.as_str())
            })
            .collect();
        assert_eq!(
            owned.len(),
            1,
            "a member owes exactly one terminal outcome, and {message_id} has {}: {completed:?}",
            owned.len()
        );
        assert_eq!(
            owned[0].get("outcome").and_then(Value::as_str),
            Some("delivered"),
            "the mailbox is bounded because entries are delivered, not because they \
             are refused: {:?}",
            owned[0]
        );
    }
}

/// What reaches the target is the artifact the mailbox holds.
///
/// The stamp is the observable that carries the claim. One clock read per entry
/// happens at intake, where the payload is built and enqueued, and three places
/// then report it: the enqueue reads it back out of the mailbox slot it just
/// filled, the envelope-metadata record is emitted from the same message, and the
/// delivered pane envelope renders it as its `Date` header. A write that rebuilt
/// its own envelope would read the clock again and the three would part — which
/// is exactly the divergence that would make this arrangement evidence about a
/// parallel artifact rather than about the one an executor will later be handed.
///
/// The slot reading is what makes this a statement about the *stored* artifact
/// rather than merely about the one built: the enqueue could otherwise store
/// something the delivery never consults, and every other observable here would
/// still agree.
///
/// The metadata record is emitted where the payload is built, so exactly one is
/// owed per task: left at the write it would describe an envelope other than the
/// stored one, and emitted at both points it would double-count.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_delivered_envelope_carries_the_stamp_the_mailbox_stored() {
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

    let inscriptions_path = inscriptions_root.join("relay.log");
    let mut message_ids = Vec::with_capacity(SENDS);
    for index in 1..=SENDS {
        let message_id = send_one(&relay_socket, index).await;
        wait_for_completed_send(&inscriptions_path, message_id.as_str()).await;
        message_ids.push(message_id);
    }

    // Read after shutdown, not before it: a duplicate write arriving late would
    // be missed by a reading taken while the relay was still running, and this
    // is the test that carries the no-message-delivered-twice claim.
    shutdown_relay_gracefully(&mut child).await;
    let envelopes = read_all_paste_buffers(temporary.path());

    let inscriptions = fs::read_to_string(&inscriptions_path).expect("read relay inscriptions");
    let metadata = inscription_details(&inscriptions, "relay.send.envelope.metadata");
    let enqueued = inscription_details(&inscriptions, "relay.delivery.mailbox.enqueued");
    let mut stamps = Vec::with_capacity(SENDS);
    for message_id in &message_ids {
        let owned: Vec<&Value> = metadata
            .iter()
            .filter(|details| {
                details.get("message_id").and_then(Value::as_str) == Some(message_id.as_str())
            })
            .collect();
        assert_eq!(
            owned.len(),
            1,
            "one envelope is built per task, so one metadata record is owed for {message_id}: \
             {metadata:?}"
        );
        let stamped = owned[0]
            .get("created_at")
            .and_then(Value::as_str)
            .expect("envelope metadata carries the stamp its payload was built with")
            .to_string();
        let stored = enqueued
            .iter()
            .find(|details| {
                details.get("message_id").and_then(Value::as_str) == Some(message_id.as_str())
            })
            .and_then(|details| details.get("payload_created_at").and_then(Value::as_str))
            .unwrap_or_else(|| {
                panic!(
                    "expected a mailbox position holding a payload for {message_id}: {enqueued:?}"
                )
            });
        assert_eq!(
            stored, stamped,
            "the payload the mailbox holds is the one the metadata record describes"
        );
        // Exactly one, not merely at least one. Each paste is loaded into its own
        // buffer under a name drawn from a per-process counter, so a second
        // delivery of one message writes a second buffer file rather than
        // overwriting the first — which makes counting them the pane-side
        // statement that nothing was delivered twice.
        let rendered: Vec<&String> = envelopes
            .iter()
            .filter(|content| content.contains(&format!("Message-Id: {message_id}")))
            .collect();
        assert_eq!(
            rendered.len(),
            1,
            "one envelope reaches the target per message, and {message_id} reached it \
             {} times: {envelopes:?}",
            rendered.len()
        );
        assert_eq!(
            header_value(rendered[0], "Date"),
            Some(stored),
            "the delivered envelope carries the stamp the mailbox stored, not one read \
             again at the write: envelope={:?}",
            rendered[0]
        );
        stamps.push(stamped);
    }

    // Distinct stamps, so the equality above is a pairing rather than a constant
    // matching itself: a build that stamped every entry from one reading would
    // satisfy every assertion so far.
    let mut distinct = stamps.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        stamps.len(),
        "each entry is stamped when it is built, so separately-built entries differ: {stamps:?}"
    );
}

/// One send's body. Shared with the assertions rather than inlined, because the
/// reservation a target reports is charged in the body's own bytes and a literal
/// repeated at the assertion would drift from the one actually sent.
fn message_body(index: usize) -> String {
    format!("mailbox shadow {index}")
}

/// Sends one message from `alpha` to itself and returns the id the relay
/// assigned it.
async fn send_one(relay_socket: &Path, index: usize) -> String {
    let response = request_relay(
        relay_socket,
        "party",
        "alpha",
        &RelayRequest::Send {
            request_id: Some(format!("req-mailbox-{index}")),
            requester_session: "alpha".to_string(),
            message: message_body(index),
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
    results[0].message_id.clone()
}

/// Waits for one message's terminal outcome, which is emitted after the
/// transition that releases its quota and retires its mailbox position.
async fn wait_for_completed_send(inscriptions_path: &Path, message_id: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let inscriptions = fs::read_to_string(inscriptions_path).unwrap_or_default();
        let completed = inscription_details(&inscriptions, "relay.send.async.completed")
            .into_iter()
            .any(|details| details.get("message_id").and_then(Value::as_str) == Some(message_id));
        if completed {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {message_id} to resolve, inscriptions={inscriptions:?}"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

/// The `details` of every inscription of one event kind, in the order the relay
/// wrote them.
fn inscription_details(inscriptions: &str, event: &str) -> Vec<Value> {
    inscriptions
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|record| record.get("event").and_then(Value::as_str) == Some(event))
        .filter_map(|record| record.get("details").cloned())
        .collect()
}

/// One RFC 822-style header's value from rendered pane-envelope text.
fn header_value<'text>(envelope: &'text str, header: &str) -> Option<&'text str> {
    let prefix = format!("{header}: ");
    envelope
        .lines()
        .find_map(|line| line.strip_prefix(prefix.as_str()))
}
