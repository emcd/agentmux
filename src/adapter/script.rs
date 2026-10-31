//! Fixture-script interpreter driving scripted turns.
//!
//! Turns follow a TOML fixture script: lifecycle events, scripted
//! `requestChoice` raises, acks per the trichotomy, snapshots for look
//! polls. Scripted `transport_ack` evidence models a fake agent effect
//! boundary, not proof that any real agent accepted input.
