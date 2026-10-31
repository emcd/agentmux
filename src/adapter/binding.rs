//! Bootstrap establishment and post-bind call discipline.
//!
//! The hello call is keyed by (spawned-connection id + direction + wire
//! id) with locally-pinned launch inputs, never by an assigned binding;
//! the hello result establishes the captured binding triple atomically.
//! After binding, requests and notifications carry the full triple
//! (validated per message) while results and errors carry none, admitted
//! only by pending-table match on captured binding plus direction plus
//! wire id. Coordinates per-type cancel/cleanup terminalization.
