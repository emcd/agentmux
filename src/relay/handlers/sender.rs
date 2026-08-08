//! Shared sender identity resolution for the target operations.
//!
//! A request's `requester_session` resolves to a [`SenderIdentity`] either from a
//! bundle member or a registered relay-wide UI session. Send, Look, Raww, and the
//! List handler all resolve their sender through [`resolve_sender_identity`].

use serde_json::json;

use crate::configuration::{
    BundleConfiguration, BundleMember, TargetConfiguration, TmuxTargetConfiguration,
};

use super::super::authorization::{AuthorizationContext, has_ui_session, ui_session_display_name};
use super::super::{RelayError, relay_error};

#[derive(Clone, Debug)]
pub(super) struct SenderIdentity {
    pub(super) session_id: String,
    pub(super) display_name: Option<String>,
}

impl SenderIdentity {
    fn from_bundle_member(member: &BundleMember) -> Self {
        Self {
            session_id: member.id.clone(),
            display_name: member.name.clone(),
        }
    }

    /// Builds the sender identity for a cross-relay ingress request: the peer
    /// relay principal itself (`<id>@RELAY`), carried as-is so the delivered
    /// envelope attributes the message to the forwarding relay. Keyed on the
    /// authenticated principal id, never a wire-supplied requester field.
    /// Original-session attribution (`on_behalf_of`) is a deferred follow-on.
    pub(super) fn relay_principal(principal_id: &str) -> Self {
        Self {
            session_id: principal_id.to_string(),
            display_name: None,
        }
    }

    pub(super) fn to_bundle_member(&self) -> BundleMember {
        BundleMember {
            id: self.session_id.clone(),
            name: self.display_name.clone(),
            working_directory: None,
            target: TargetConfiguration::Tmux(TmuxTargetConfiguration {
                start_command: "ui-session".to_string(),
                prompt_readiness: None,
            }),
            coder_session_id: None,
            policy_id: None,
            environment: Vec::new(),
        }
    }
}

/// Resolves a requester to a [`SenderIdentity`] in its home namespace.
///
/// A bundle-bound home resolves the sender from the bundle's members (falling
/// back to a registered relay-wide UI session via [`resolve_sender_identity`]); a
/// relay-wide home (`GLOBAL`, with no bundle) resolves it purely from the
/// registered UI sessions in the operator authorization.
pub(super) fn resolve_sender_in_namespace(
    home_bundle: Option<&BundleConfiguration>,
    authorization: &AuthorizationContext,
    requester_session: &str,
    detail_field: &str,
) -> Result<SenderIdentity, RelayError> {
    match home_bundle {
        Some(home_bundle) => {
            resolve_sender_identity(home_bundle, authorization, requester_session, detail_field)
        }
        None if has_ui_session(authorization, requester_session) => Ok(SenderIdentity {
            session_id: requester_session.to_string(),
            display_name: ui_session_display_name(authorization, requester_session)
                .map(ToString::to_string),
        }),
        None => Err(relay_error(
            "validation_unknown_sender",
            "sender session is not configured",
            Some(json!({
                "field": detail_field,
                "value": requester_session,
            })),
        )),
    }
}

pub(super) fn resolve_sender_identity(
    bundle: &BundleConfiguration,
    authorization: &AuthorizationContext,
    sender_session: &str,
    detail_field: &str,
) -> Result<SenderIdentity, RelayError> {
    if let Some(member) = bundle
        .members
        .iter()
        .find(|member| member.id == sender_session)
    {
        return Ok(SenderIdentity::from_bundle_member(member));
    }
    if has_ui_session(authorization, sender_session) {
        return Ok(SenderIdentity {
            session_id: sender_session.to_string(),
            display_name: ui_session_display_name(authorization, sender_session)
                .map(ToString::to_string),
        });
    }
    Err(relay_error(
        "validation_unknown_sender",
        "sender session is not configured",
        Some(json!({
            "field": detail_field,
            "value": sender_session,
        })),
    ))
}
