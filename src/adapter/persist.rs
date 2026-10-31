//! `adapter.persistState` writes.
//!
//! Tag-echoed writes under the current generation with client-side
//! duplicate-id tracking and an explicit adapter-local finite cap on
//! outstanding entries. Never checks currency and never touches the
//! commit pointer — admission and commit authority stay supervisor-side.
