//! ACP generation-fence tests in their own binary.
//!
//! These tests deliver real SIGTERM to the test process and share a
//! process-wide inscription sink, so they cannot run in the same process
//! as the rest of the suite: a fence SIGTERM would trip the global
//! shutdown flag out from under concurrently running tests, and the
//! shared sink requires serial execution anyway. A separate binary gives
//! them a process of their own; run it with `-- --test-threads=1`.

// Shared with the main integration binary, which exercises the helpers
// this crate does not use.
#[allow(dead_code, unused_imports)]
#[path = "integration/acp/helpers/mod.rs"]
mod helpers;

#[path = "integration/acp/generation_fence/mod.rs"]
mod generation_fence;
