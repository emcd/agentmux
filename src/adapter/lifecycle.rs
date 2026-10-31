//! Lifecycle events, ping liveness, and the diagnostic sink.
//!
//! Emits ready/degraded/failed events with namespaced typed codes
//! without dictating supervision outcomes; coalesces or drops only
//! progress/heartbeat under flood while authoritative transitions are
//! never dropped. Answers `relay.ping` with the empty-object result and
//! tracks liveness from valid frames. Owns the local diagnostic and
//! inscription sink for shed gate-drops and cleanup records — structured
//! records, never stdout, so diagnostics cannot corrupt the frame stream.
