//! `adapter.requestChoice` raising and originated-call discipline.
//!
//! Generation-bound requests with originator-monotonic wire ids from 1,
//! pending-table admission, resolver blocking limited to the adapter's
//! own turn, dual-cancel on disconnect, record-before-respond, and the
//! negotiated in-flight cap with typed local shed (inscribed, never
//! queued). Owns the pending-call table for calls the adapter originates.
