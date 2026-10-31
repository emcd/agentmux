//! NDJSON framing, outbox bounds, and the control lane.
//!
//! Reads newline-delimited UTF-8 JSON-RPC 2.0 frames, bounding the
//! accumulated inbound bytes before the terminating newline (no length
//! prefix on the wire): the 64KiB bootstrap ceiling before negotiation,
//! the negotiated `max_frame_bytes` as the operative bound after. Owns
//! the transmit queue caps with the perspective rule, the bounded drain
//! wait, and the control lane exempt from data caps within its own bound.
