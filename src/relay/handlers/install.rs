//! Peer credential install operation (`InstallPeerCredential`).
//!
//! Writes an issued PSK into this relay's relay-owned peer slot under the
//! shared identity-admin serialization, with alias-referent binding,
//! three-way slot-state authorization, and confined publication.

use std::path::Path;

use crate::configuration::ConfigurationRoots;
use crate::relay::authorization::{RelayActionFamily, authorize_relay_action};
use crate::relay::confined::{ConfineError, confined_read, confined_stage};
use crate::relay::identity::{
    PrincipalStore, PrincipalType, invalid_credential_path, state_relative_path,
};
use crate::relay::{RelayError, RelayResponse, SCHEMA_VERSION, relay_error};
use crate::runtime::paths::peer_relay_psk_path;

/// Installs a peer credential into this relay's relay-owned peer slot.
///
/// Writes the raw PSK (issued by the opposite relay) to
/// `peers/<alias>.psk` with mode 0600, creating missing relay-owned
/// directories with mode 0700, and never returns the PSK. Requires
/// `<alias>@RELAY` registered as a relay principal (the alias referent), so
/// no orphan credential file can exist. Slot-state authorization classifies
/// at commit time under the shared identity-admin serialization: absent
/// requires `new.peer=all`, byte-identical content accepts either control
/// (so a lost-response retry succeeds for the original caller), and
/// different content requires `change.psk=all`.
///
/// Every filesystem step traverses the state root without following
/// symlinks; publication is temp-write plus file fsync, atomic rename, then
/// directory fsync, with a typed durability-uncertain outcome past the
/// rename point.
pub(in crate::relay) fn handle_install_peer_credential(
    configuration_roots: &ConfigurationRoots,
    state_root: &Path,
    requester_principal_id: &str,
    alias: String,
    psk: String,
) -> Result<RelayResponse, RelayError> {
    if !is_valid_peer_alias(alias.as_str()) {
        return Err(invalid_credential_path(
            alias.as_str(),
            "peer alias is not a safe path component",
        ));
    }
    if psk.is_empty() {
        return Err(relay_error(
            "validation_invalid_params",
            "psk must not be empty for peer credential install",
            Some(serde_json::json!({ "alias": alias })),
        ));
    }
    let store = PrincipalStore::load(state_root)?;
    let referent = store.find_by_principal_id(format!("{alias}@RELAY").as_str());
    if !referent.is_some_and(|record| record.principal_type == PrincipalType::Relay) {
        return Err(relay_error(
            "validation_unknown_principal",
            "no relay principal registered for alias; register it with new peer first",
            Some(serde_json::json!({ "alias": alias })),
        ));
    }
    let slot_path = peer_relay_psk_path(state_root, alias.as_str());
    let relative = state_relative_path(state_root, &slot_path);
    let slot = match confined_read(state_root, relative) {
        Ok(None) => SlotState::Absent,
        Ok(Some(current)) if current == psk.as_bytes() => SlotState::Identical,
        Ok(Some(_)) => SlotState::Different,
        Err(ConfineError::Symlink { component }) => {
            return Err(invalid_credential_path(
                component.as_str(),
                "peer credential ancestor is a symlink",
            ));
        }
        Err(other) => {
            return Err(install_io_error(&slot_path, &other));
        }
    };
    match slot {
        SlotState::Absent => {
            authorize_relay_action(
                configuration_roots,
                requester_principal_id,
                RelayActionFamily::New,
                "peer",
            )?;
        }
        SlotState::Identical => {
            // Either control suffices: an identical rewrite changes nothing,
            // so accepting both preserves lost-response retry for the
            // original caller without widening authority. Surface the
            // provisioning denial when neither control is held.
            if let Err(new_denial) = authorize_relay_action(
                configuration_roots,
                requester_principal_id,
                RelayActionFamily::New,
                "peer",
            ) && authorize_relay_action(
                configuration_roots,
                requester_principal_id,
                RelayActionFamily::Change,
                "psk",
            )
            .is_err()
            {
                return Err(new_denial);
            }
        }
        SlotState::Different => {
            authorize_relay_action(
                configuration_roots,
                requester_principal_id,
                RelayActionFamily::Change,
                "psk",
            )?;
        }
    }
    let staged =
        confined_stage(state_root, relative, psk.as_bytes(), "cred").map_err(
            |error| match error {
                ConfineError::Symlink { component } => invalid_credential_path(
                    component.as_str(),
                    "peer credential ancestor is a symlink",
                ),
                ConfineError::Exchanged { component } => invalid_credential_path(
                    component.as_str(),
                    "peer credential ancestor changed during commit",
                ),
                _ => install_io_error(&slot_path, &error),
            },
        )?;
    let written_path = staged.commit().map_err(|error| match error {
        ConfineError::Symlink { component } => {
            invalid_credential_path(component.as_str(), "peer credential ancestor is a symlink")
        }
        ConfineError::Exchanged { component } => invalid_credential_path(
            component.as_str(),
            "peer credential ancestor changed during commit",
        ),
        ConfineError::DirSync { source } => relay_error(
            "internal_peer_credential_durability_uncertain",
            "peer credential published but parent-directory sync failed; durability is uncertain",
            Some(serde_json::json!({
                "path": slot_path.display().to_string(),
                "cause": source.to_string(),
            })),
        ),
        ConfineError::Io { .. } => install_io_error(&slot_path, &error),
    })?;
    Ok(RelayResponse::InstallPeerCredential {
        schema_version: SCHEMA_VERSION.to_string(),
        alias,
        written_path,
    })
}

/// True when `alias` is safe to embed in the relay-owned peer slot filename:
/// non-empty, with no path separator, no identity qualifier, and no
/// traversal-only value. Safe grammar does not protect a symlinked `peers/`
/// ancestor — that is the traversal helper's job — but it keeps a crafted
/// alias from escaping the slot directory lexically.
fn is_valid_peer_alias(alias: &str) -> bool {
    !alias.is_empty()
        && alias != "."
        && alias != ".."
        && !alias.contains('/')
        && !alias.contains('@')
}

/// Slot classification for peer credential install authorization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotState {
    Absent,
    Identical,
    Different,
}

fn install_io_error(slot_path: &Path, error: &ConfineError) -> RelayError {
    relay_error(
        "internal_peer_credential_install",
        "failed to install peer credential",
        Some(serde_json::json!({
            "path": slot_path.display().to_string(),
            "cause": format!("{error:?}"),
        })),
    )
}
