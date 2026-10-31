//! `relay.look` poll responses.
//!
//! Snapshot entries with full freshness metadata from the generation
//! the snapshot was read from, adapter-side prime-wait bounded strictly
//! by the relay-supplied timeout (zero means no wait), and synthetic
//! stale-empty snapshots for fence terminalization. Polls proceed
//! concurrently within `max_looks_in_flight`, never blocked on a turn.
