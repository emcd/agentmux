//! Reading the head of a target's mailbox without advancing anything.

use crate::protocol::mailbox::{MailboxEntry, MailboxEntryKind};
use crate::protocol::operations::{PeekRejection, PeekRequest, PeekResponse, PeekResult};

use super::super::ledger::lock_ledger;
use super::addressing::target_key;
use super::generation::active_generation;

/// Reads the head of a target's mailbox, advancing nothing.
///
/// Repeatable by construction: it copies out of the mailbox and writes nothing
/// back, so two calls with no acknowledgment between them return the same run.
///
/// The run stops at the first entry that would breach either bound, at the first
/// gap in the numbering, and before any raw entry — except when a raw entry is
/// itself at the head, in which case it is returned alone. Returning it alone
/// rather than skipping past it is what keeps a raw entry from parking a mailbox
/// permanently behind a bound too small to admit it beside mail.
pub(in crate::relay) fn peek(request: &PeekRequest) -> PeekResult {
    let Ok(state) = lock_ledger() else {
        return Err(PeekRejection::UnknownTarget);
    };
    let target = target_key(&request.binding.target);
    let Some(mailbox) = state.mailboxes.get(&target) else {
        return Err(PeekRejection::UnknownTarget);
    };
    if active_generation(&state, &target) != Some(request.binding.generation) {
        return Err(PeekRejection::GenerationSuperseded);
    }

    let mut entries: Vec<MailboxEntry> = Vec::new();
    let mut bytes_total: u64 = 0;
    let mut expected = mailbox.cursor.next_sequence();
    while entries.len() < request.entry_max {
        let Some(slot) = mailbox.slots.get(&expected) else {
            break;
        };
        let Some(admitted) = state.entries.get(slot.message_id.as_str()) else {
            break;
        };
        let kind = slot.payload.kind();
        // A raw entry joins nothing. At the head it is the whole answer; after
        // mail it ends the run, so mail is never reordered around it and it is
        // never coalesced into the mail behind it.
        if kind == MailboxEntryKind::Raw && !entries.is_empty() {
            break;
        }
        // The head entry is admitted whatever it costs. Enforcing the byte bound
        // against it would let one entry larger than the caller's budget park
        // every entry behind it indefinitely — the same permanent park the
        // raw-singleton rule exists to prevent, arrived at from the other side.
        let next_total = bytes_total.saturating_add(admitted.canonical_bytes);
        if !entries.is_empty() && next_total > request.canonical_bytes_max {
            break;
        }
        bytes_total = next_total;
        entries.push(MailboxEntry {
            sequence: expected,
            message_id: slot.message_id.clone(),
            canonical_bytes: admitted.canonical_bytes,
            payload: slot.payload.clone(),
        });
        if kind == MailboxEntryKind::Raw {
            break;
        }
        expected = expected.next();
    }

    Ok(PeekResponse {
        entries,
        cursor: mailbox.cursor,
    })
}

/// What bounds the run a peek returns, and what leaves it untouched.
///
/// Inline because the mailbox is reached through a process-global ledger behind
/// one crate-private lock, and `peek` is `pub(in crate::relay)` by design — the
/// public seam for transports is the delivery-loop executor contract, and
/// widening these operations to reach them from `tests/` would publish the
/// delivery ledger itself. No public interface exercises them at all until the
/// executors that call them exist.
///
/// One test rather than five because the property is a single one stated over
/// several boundaries: a peek reports the head run and changes nothing. Each
/// boundary below is a different way that run can end, and asserting them apart
/// would let a change that collapsed two of them into each other still pass.
#[cfg(test)]
mod mailbox_peek_tests {
    use super::super::fixtures::{binding, claim, mail, peeked, place, raw, request};
    use super::*;

    #[test]
    fn a_peek_reports_the_head_run_and_changes_nothing() {
        let namespace = "mbx-peek";
        let bound = claim(namespace);
        for index in 1..=3 {
            place(namespace, &format!("{namespace}-{index}"), 10, mail("body"));
        }

        assert_eq!(
            peeked(&bound, 10, 1_000),
            vec![1, 2, 3],
            "an unbounded peek reports the whole contiguous mail run"
        );
        // Repeatable: the same call twice with no acknowledgment between must
        // report the same run, which is what makes a peek safe to retry after a
        // transport decides not to write what it saw.
        assert_eq!(
            peeked(&bound, 10, 1_000),
            vec![1, 2, 3],
            "peeking twice without an acknowledgment reports the same run"
        );
        assert_eq!(
            peeked(&bound, 2, 1_000),
            vec![1, 2],
            "the entry bound truncates the run"
        );
        // The head is admitted whatever it costs — 10 bytes against a 5-byte
        // budget still comes back — because refusing it would park every entry
        // behind it forever. The bound applies from the second entry on.
        assert_eq!(
            peeked(&bound, 10, 5),
            vec![1],
            "the byte bound never withholds the head entry"
        );
        assert_eq!(
            peeked(&bound, 10, 15),
            vec![1],
            "the byte bound truncates before the entry that would breach it"
        );

        // A raw entry at the head is the whole answer, so a bound too small to
        // carry it beside mail cannot park it.
        let raw_head = "mbx-peek-raw";
        let raw_bound = claim(raw_head);
        place(raw_head, "mbx-peek-raw-1", 10, raw("input"));
        place(raw_head, "mbx-peek-raw-2", 10, mail("body"));
        assert_eq!(
            peeked(&raw_bound, 10, 1_000),
            vec![1],
            "a raw head entry is returned alone even when mail behind it would fit"
        );

        // A raw entry behind mail ends the run instead of joining it, so mail is
        // never reordered around it.
        let barrier = "mbx-peek-barrier";
        let barrier_bound = claim(barrier);
        place(barrier, "mbx-peek-barrier-1", 10, mail("body"));
        place(barrier, "mbx-peek-barrier-2", 10, raw("input"));
        place(barrier, "mbx-peek-barrier-3", 10, mail("body"));
        assert_eq!(
            peeked(&barrier_bound, 10, 1_000),
            vec![1],
            "the run stops before a raw entry rather than coalescing it into mail"
        );

        // A generation that is not the target's returns nothing at all, which is
        // distinct from returning an empty run.
        assert_eq!(
            peek(&request(
                &binding(namespace, bound.generation.value() + 1),
                10,
                1_000
            ))
            .unwrap_err(),
            PeekRejection::GenerationSuperseded,
            "a generation the target does not hold is refused rather than handed an empty run"
        );
    }
}
