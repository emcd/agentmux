//! Choice-resolution waiter state.
//!
//! Per-request channel shared between the producer that resolves the choice
//! (the relay's resolution path / ACP respawn path) and the consumer awaiting
//! the result (the in-flight `wait_for_choice_resolution` future spawned for
//! each choice request). A waiter is registered just after the queue records
//! the request and consumed (`take_waiter`) when the resolution completes.
//!
//! Waiter helpers, in declaration order:
//! - `choice_waiters` accessor for the underlying `OnceLock<Mutex<...>>`.
//! - `register_waiter` inserts a fresh `(Mutex, Condvar)` pair keyed by
//!   `choice_request_id`.
//! - `get_waiter` returns a clone of the waiter's `Arc` so the consumer can
//!   park on its condvar.
//! - `take_waiter` removes the waiter, signaling that resolution has completed.
//! - `timestamp_rfc3339` formats the current UTC time as RFC 3339 for the
//!   choice-request and resolution inscriptions.

use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, OnceLock},
};

use time::format_description::well_known::Rfc3339;

use super::SharedWaiterState;

pub(super) static CHOICE_WAITERS: OnceLock<Mutex<HashMap<String, SharedWaiterState>>> =
    OnceLock::new();

fn choice_waiters() -> &'static Mutex<HashMap<String, SharedWaiterState>> {
    CHOICE_WAITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn register_waiter(choice_request_id: &str) -> Result<(), String> {
    let mut waiters = choice_waiters()
        .lock()
        .map_err(|_| "failed to lock choice waiters".to_string())?;
    waiters.insert(
        choice_request_id.to_string(),
        Arc::new((Mutex::new(None), Condvar::new())),
    );
    Ok(())
}

pub(super) fn get_waiter(choice_request_id: &str) -> Result<Option<SharedWaiterState>, String> {
    let waiters = choice_waiters()
        .lock()
        .map_err(|_| "failed to lock choice waiters".to_string())?;
    Ok(waiters.get(choice_request_id).cloned())
}

pub(super) fn take_waiter(choice_request_id: &str) -> Result<Option<SharedWaiterState>, String> {
    let mut waiters = choice_waiters()
        .lock()
        .map_err(|_| "failed to lock choice waiters".to_string())?;
    Ok(waiters.remove(choice_request_id))
}

pub(super) fn timestamp_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}
