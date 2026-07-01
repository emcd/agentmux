//! Authorization, split into behavioral seams over shared domain types.
//!
//! This module is an import-only hub. [`context`] holds the shared types
//! ([`AuthorizationContext`] and the policy primitives), and the three seams act
//! on them:
//!
//! - [`loading`]: parse `policies.toml` / `relay.toml` into validated presets and
//!   build an [`AuthorizationContext`].
//! - [`resolution`]: map a requester (a bundle-member session or a relay-wide
//!   principal) to its resolved policy controls, plus the UI-session accessors.
//! - [`checks`]: the uniform `authorize_*` decisions over a resolved route or a
//!   relay-wide operator action.
//!
//! Nothing is defined here — the root only wires submodules and re-exports the
//! relay-facing API.

mod checks;
mod context;
mod loading;
mod resolution;

pub(in crate::relay) use checks::{
    RelayActionFamily, RouteAuthorization, authorize_choose, authorize_choose_for_list,
    authorize_relay_action, authorize_route, authorize_updown, reject_cross_relay_ingress,
};
pub(in crate::relay) use context::AuthorizationContext;
pub(in crate::relay) use loading::load_authorization_context;
pub use loading::{
    PeerConfiguration, RelayRuntimeConfiguration, load_relay_runtime_configuration,
    parse_relay_bool_env_value, resolve_relay_bool_setting,
};
pub(in crate::relay) use resolution::{
    choices_pending_max, choose_authorized_ui_sessions, has_ui_session, ui_session_display_name,
};
