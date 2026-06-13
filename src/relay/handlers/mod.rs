//! Request handlers, one operation per submodule over shared sender resolution.
//!
//! This module is an import-only hub: [`dispatch`] holds the per-bundle router
//! and the relay-wide entry points; [`sender`] holds the shared sender-identity
//! resolution; and each operation lives in its own submodule (`send`, `look`,
//! `raww`, `listing`, `identity`, `choices`). Nothing is defined here — the
//! root only wires submodules and re-exports the relay-facing API.

mod choices;
mod dispatch;
mod identity;
mod listing;
mod look;
mod raww;
mod routed;
mod send;
mod sender;

pub(in crate::relay) use dispatch::{
    build_identity_snapshot_event, emit_choices_snapshot_for_ui_registration, handle_global_list,
    handle_identity_admin_request, handle_identity_introspect, handle_request,
};
pub(in crate::relay) use listing::handle_list_routed;
pub(in crate::relay) use look::handle_look_routed;
pub(in crate::relay) use raww::handle_raww_routed;
pub(in crate::relay) use send::handle_send_routed;
