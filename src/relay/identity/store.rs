use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::json;
use subtle::ConstantTimeEq;
use time::OffsetDateTime;

use crate::relay::{
    RelayError,
    confined::{ConfineError, confined_read, confined_stage},
    relay_error,
};
use crate::runtime::paths::principal_store_path;

use super::parse_peer_scope;
use super::paths::{invalid_credential_path, state_relative_path};
use super::verify::record_is_expired;

const PRINCIPAL_STORE_FORMAT_VERSION: u32 = 1;

/// Categorizes a principal by namespace partition.
///
/// The variant is derived from the `<id>@<namespace>` portion of a
/// `principal_id`. Capability gating uses this type to decide which request
/// surfaces a Hello-authenticated connection may invoke.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PrincipalType {
    Session,
    User,
    Application,
    Relay,
}

impl PrincipalType {
    /// Returns the snake_case wire token for this principal type.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::User => "user",
            Self::Application => "application",
            Self::Relay => "relay",
        }
    }
}

/// Persisted record for one registered principal.
///
/// `credential_hash` is the lowercase hex SHA-256 of the raw PSK; the raw PSK
/// never appears here. `scope` is meaningful for `Application` and `Relay`
/// principals and is set at registration time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PrincipalRecord {
    pub(crate) principal_id: String,
    pub(crate) principal_type: PrincipalType,
    pub(crate) credential_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) metadata: HashMap<String, String>,
}

impl PrincipalRecord {
    /// Returns true when this record's `expires_at` has passed (or cannot be
    /// parsed; fail-closed). A record with no `expires_at` never expires. Used
    /// by identity introspection to report `verified` without mutating the
    /// store (an expired record must still surface as `verified: false`).
    pub(crate) fn is_expired(&self, now: OffsetDateTime) -> bool {
        record_is_expired(self, now)
    }
}

/// Relay-level principal store backed by `<state-root>/identity/principals.json`.
///
/// Loads at relay startup; writes are performed atomically with restrictive
/// mode (0600) on every mutation. Both load and persist traverse the state
/// root without following symlinks (see [`confined`]).
#[derive(Clone, Debug, Default)]
pub(crate) struct PrincipalStore {
    path: PathBuf,
    state_root: PathBuf,
    records_by_hash: HashMap<String, PrincipalRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreEnvelope {
    format_version: u32,
    #[serde(default)]
    principals: Vec<PrincipalRecord>,
}

impl PrincipalStore {
    /// Loads the principal store under `state_root`, returning an empty store
    /// when the file does not yet exist. A symlinked ancestor aborts with
    /// `validation_invalid_credential_path` instead of reading outside the
    /// state tree.
    pub(crate) fn load(state_root: &Path) -> Result<Self, RelayError> {
        let path = principal_store_path(state_root);
        let relative = state_relative_path(state_root, &path);
        let raw = match confined_read(state_root, relative) {
            Ok(Some(bytes)) => String::from_utf8(bytes).map_err(|source| {
                relay_error(
                    "internal_principal_store",
                    "failed to read principal store",
                    Some(json!({
                        "path": path.display().to_string(),
                        "cause": source.to_string(),
                    })),
                )
            })?,
            Ok(None) => {
                return Ok(Self {
                    path,
                    state_root: state_root.to_path_buf(),
                    records_by_hash: HashMap::new(),
                });
            },
            Err(ConfineError::Symlink { component }) => {
                return Err(invalid_credential_path(
                    component.as_str(),
                    "principal store ancestor is a symlink",
                ));
            },
            Err(other) => {
                let source = other.io_source().unwrap_or_else(|| {
                    io::Error::other("confined store read failed")
                });
                return Err(relay_error(
                    "internal_principal_store",
                    "failed to read principal store",
                    Some(json!({
                        "path": path.display().to_string(),
                        "cause": source.to_string(),
                    })),
                ));
            },
        };
        let envelope: StoreEnvelope =
            serde_json::from_str(&raw).map_err(|source| {
                relay_error(
                    "internal_principal_store",
                    "failed to parse principal store",
                    Some(json!({
                        "path": path.display().to_string(),
                        "cause": source.to_string(),
                    })),
                )
            })?;
        if envelope.format_version != PRINCIPAL_STORE_FORMAT_VERSION {
            return Err(relay_error(
                "internal_principal_store",
                "principal store has unsupported format-version",
                Some(json!({
                    "path": path.display().to_string(),
                    "format_version": envelope.format_version,
                    "supported": PRINCIPAL_STORE_FORMAT_VERSION,
                })),
            ));
        }
        let mut records_by_hash =
            HashMap::with_capacity(envelope.principals.len());
        for record in envelope.principals {
            if record.principal_type == PrincipalType::Relay
                && parse_peer_scope(record.scope.as_deref()).is_err()
            {
                return Err(relay_error(
                    "internal_principal_store",
                    "principal store holds a malformed peer scope",
                    Some(json!({
                        "path": path.display().to_string(),
                        "principal_id": record.principal_id,
                    })),
                ));
            }
            records_by_hash.insert(record.credential_hash.clone(), record);
        }
        Ok(Self {
            path,
            state_root: state_root.to_path_buf(),
            records_by_hash,
        })
    }

    /// Looks up a principal by SHA-256 hex `credential_hash` using a
    /// constant-time per-key comparison to avoid token-equality timing leaks.
    pub(crate) fn find_by_credential_hash(
        &self,
        hash_hex: &str,
    ) -> Option<&PrincipalRecord> {
        let probe = hash_hex.as_bytes();
        for (stored_hash, record) in &self.records_by_hash {
            if stored_hash.len() == probe.len()
                && bool::from(stored_hash.as_bytes().ct_eq(probe))
            {
                return Some(record);
            }
        }
        None
    }

    /// Iterates every loaded principal record in arbitrary order, without expiry
    /// filtering. Callers that need only active records filter on
    /// [`PrincipalRecord::is_expired`].
    pub(crate) fn records(&self) -> impl Iterator<Item = &PrincipalRecord> {
        self.records_by_hash.values()
    }

    /// Looks up a principal by its registered `principal_id`.
    pub(crate) fn find_by_principal_id(
        &self,
        principal_id: &str,
    ) -> Option<&PrincipalRecord> {
        self.records_by_hash
            .values()
            .find(|record| record.principal_id == principal_id)
    }

    /// Inserts or replaces a principal record by `credential_hash`.
    ///
    /// Returns any prior record displaced by the insert; rotation uses this
    /// to surface the old hash for revocation dispatch (Slice 2).
    pub(crate) fn insert(
        &mut self,
        record: PrincipalRecord,
    ) -> Option<PrincipalRecord> {
        self.records_by_hash
            .insert(record.credential_hash.clone(), record)
    }

    /// Removes records whose `expires_at` is at or before `now`, plus records
    /// whose `expires_at` cannot be parsed as RFC 3339 (fail-closed: a corrupt
    /// expiry must not authenticate). Returns the number of records pruned.
    pub(crate) fn prune_expired(&mut self, now: OffsetDateTime) -> usize {
        let before = self.records_by_hash.len();
        self.records_by_hash
            .retain(|_, record| !record_is_expired(record, now));
        before - self.records_by_hash.len()
    }

    /// Removes a principal by `principal_id` regardless of credential hash.
    pub(crate) fn remove_by_principal_id(
        &mut self,
        principal_id: &str,
    ) -> Option<PrincipalRecord> {
        let key = self
            .records_by_hash
            .iter()
            .find(|(_, record)| record.principal_id == principal_id)
            .map(|(hash, _)| hash.clone())?;
        self.records_by_hash.remove(&key)
    }

    /// Writes the principal store to disk with mode 0600.
    ///
    /// Persists by writing to a sibling temporary file, syncing it, renaming
    /// over the store file (the update linearization point for effective state),
    /// then syncing the parent directory before reporting durable success. A
    /// post-rename directory-sync failure reports
    /// `internal_store_durability_uncertain`: publication already occurred, so
    /// callers must not claim the old store remains durable or effective.
    /// Every step traverses the state root without following symlinks; a
    /// symlinked ancestor aborts with `validation_invalid_credential_path`.
    pub(crate) fn persist(&self) -> Result<(), RelayError> {
        let envelope = StoreEnvelope {
            format_version: PRINCIPAL_STORE_FORMAT_VERSION,
            principals: self.records_by_hash.values().cloned().collect(),
        };
        let serialized =
            serde_json::to_vec_pretty(&envelope).map_err(|source| {
                relay_error(
                    "internal_principal_store",
                    "failed to serialize principal store",
                    Some(json!({
                        "path": self.path.display().to_string(),
                        "cause": source.to_string(),
                    })),
                )
            })?;
        // Stage into a per-attempt-unique sibling and enforce mode BEFORE the
        // rename. A failure before publication leaves the old store intact.
        let relative = state_relative_path(&self.state_root, &self.path);
        let staged = confined_stage(
            &self.state_root,
            relative,
            serialized.as_slice(),
            "store",
        )
        .map_err(|error| match error {
            ConfineError::Symlink { component } => invalid_credential_path(
                component.as_str(),
                "principal store ancestor is a symlink",
            ),
            ConfineError::Exchanged { component } => invalid_credential_path(
                component.as_str(),
                "principal store ancestor changed during commit",
            ),
            ConfineError::DirSync { .. } => self.io_error(
                "stage store",
                io::Error::other("unexpected directory sync during staging"),
            ),
            ConfineError::Io { source } => {
                self.io_error("stage store", source)
            },
        })?;
        // Commit point: the rename publishes the replacement.
        staged.commit().map(|_| ()).map_err(|error| match error {
            ConfineError::Symlink { component } => invalid_credential_path(
                component.as_str(),
                "principal store ancestor is a symlink",
            ),
            ConfineError::Exchanged { component } => invalid_credential_path(
                component.as_str(),
                "principal store ancestor changed during commit",
            ),
            ConfineError::DirSync { source } => relay_error(
                "internal_store_durability_uncertain",
                "principal store published but parent-directory sync failed; durability is uncertain",
                Some(json!({
                    "path": self.path.display().to_string(),
                    "context": "sync parent",
                    "cause": source.to_string(),
                })),
            ),
            ConfineError::Io { source } => self.io_error("rename store", source),
        })
    }

    fn io_error(&self, context: &str, source: io::Error) -> RelayError {
        relay_error(
            "internal_principal_store",
            "principal store io failure",
            Some(json!({
                "path": self.path.display().to_string(),
                "context": context,
                "cause": source.to_string(),
            })),
        )
    }
}
