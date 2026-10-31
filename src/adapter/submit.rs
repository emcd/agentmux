//! `adapter.submit` execution and ack trichotomy.
//!
//! Single-flight turn gate with `RefusedBusy` as positive no-effect
//! evidence naming adapter single-flight; terminal outcomes use the
//! contract literals (`Submitted` / `TransportUnavailable` /
//! `SubmissionUnknown`) with `evidence_basis` on unknown and no-effect
//! outcomes. The gate limits turns, never Look/control progress.
