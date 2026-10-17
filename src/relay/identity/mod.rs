mod credentials;
mod paths;
mod session;
mod store;
mod verify;

pub(crate) use credentials::{
    generate_psk, stage_credential_sink, write_pending_credential,
};
pub(crate) use paths::{invalid_credential_path, state_relative_path};
pub(in crate::relay) use session::{bare_session_id, canonical_session_id};
pub(crate) use store::{PrincipalRecord, PrincipalStore, PrincipalType};
pub(crate) use verify::{
    IdentityIntrospectRights, SOCKET_TRUST_TOKEN, VerifiedIdentity,
    classify_principal_id, hash_token_sha256, scope_permits,
    split_principal_id, verify_hello_credential,
};

pub(crate) use super::peer_scope::{
    is_wildcard_scope, live_peer_ingress, parse_peer_scope,
    peer_scope_covers_namespace, peer_scope_covers_target,
};
