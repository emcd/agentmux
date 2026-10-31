//! `handshakeHello` version and limits negotiation.
//!
//! Major equality plus minor-window overlap (selected_minor is the min of
//! maxes within the overlap), matched-pairs limit resolution (never
//! same-named min by accident), mandatory `latency_declarations`, and
//! deterministic typed refusal on any mismatch — no degraded operation.
