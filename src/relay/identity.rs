use std::{
    collections::HashMap,
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

use base64::Engine;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{GLOBAL_SESSION_SUFFIX, RelayError, relay_error};

const PSK_BYTE_LENGTH: usize = 32;
const PRINCIPAL_FILE_MODE: u32 = 0o600;
const PRINCIPAL_STORE_FORMAT_VERSION: u32 = 1;

/// Returns the canonical `session@bundle` identity for a session id.
///
/// Global-user identities already carry the `@GLOBAL` suffix and are their own
/// canonical form; bundle-local identities are qualified with the bundle name.
pub(super) fn canonical_session_id(session_id: &str, bundle_name: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        session_id.to_string()
    } else {
        format!("{session_id}@{bundle_name}")
    }
}

/// Returns the bundle-local session id for a possibly-canonical identity.
///
/// Strips a trailing `@{bundle_name}` qualifier so internal lookups match
/// configured member ids; global-user (`@GLOBAL`) identities and already-bare
/// ids are returned unchanged.
pub(super) fn bare_session_id(session_id: &str, bundle_name: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        return session_id.to_string();
    }
    let qualifier = format!("@{bundle_name}");
    session_id
        .strip_suffix(qualifier.as_str())
        .unwrap_or(session_id)
        .to_string()
}

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
/// mode (0600) on every mutation.
#[derive(Clone, Debug, Default)]
pub(crate) struct PrincipalStore {
    path: PathBuf,
    records_by_hash: HashMap<String, PrincipalRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StoreEnvelope {
    format_version: u32,
    #[serde(default)]
    principals: Vec<PrincipalRecord>,
}

impl PrincipalStore {
    /// Loads the principal store at `path`, returning an empty store when the
    /// file does not yet exist.
    pub(crate) fn load(path: PathBuf) -> Result<Self, RelayError> {
        let raw = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Self {
                    path,
                    records_by_hash: HashMap::new(),
                });
            }
            Err(source) => {
                return Err(relay_error(
                    "internal_principal_store",
                    "failed to read principal store",
                    Some(json!({
                        "path": path.display().to_string(),
                        "cause": source.to_string(),
                    })),
                ));
            }
        };
        let envelope: StoreEnvelope = serde_json::from_str(&raw).map_err(|source| {
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
        let mut records_by_hash = HashMap::with_capacity(envelope.principals.len());
        for record in envelope.principals {
            records_by_hash.insert(record.credential_hash.clone(), record);
        }
        Ok(Self {
            path,
            records_by_hash,
        })
    }

    /// Looks up a principal by SHA-256 hex `credential_hash` using a
    /// constant-time per-key comparison to avoid token-equality timing leaks.
    pub(crate) fn find_by_credential_hash(&self, hash_hex: &str) -> Option<&PrincipalRecord> {
        let probe = hash_hex.as_bytes();
        for (stored_hash, record) in &self.records_by_hash {
            if stored_hash.len() == probe.len() && bool::from(stored_hash.as_bytes().ct_eq(probe)) {
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
    pub(crate) fn find_by_principal_id(&self, principal_id: &str) -> Option<&PrincipalRecord> {
        self.records_by_hash
            .values()
            .find(|record| record.principal_id == principal_id)
    }

    /// Inserts or replaces a principal record by `credential_hash`.
    ///
    /// Returns any prior record displaced by the insert; rotation uses this
    /// to surface the old hash for revocation dispatch (Slice 2).
    pub(crate) fn insert(&mut self, record: PrincipalRecord) -> Option<PrincipalRecord> {
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
    pub(crate) fn remove_by_principal_id(&mut self, principal_id: &str) -> Option<PrincipalRecord> {
        let key = self
            .records_by_hash
            .iter()
            .find(|(_, record)| record.principal_id == principal_id)
            .map(|(hash, _)| hash.clone())?;
        self.records_by_hash.remove(&key)
    }

    /// Writes the principal store to disk with mode 0600.
    ///
    /// Persists by writing to a sibling temporary file and renaming so that a
    /// crash mid-write cannot corrupt an existing store.
    pub(crate) fn persist(&self) -> Result<(), RelayError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|source| self.io_error("create parent", source))?;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
        let envelope = StoreEnvelope {
            format_version: PRINCIPAL_STORE_FORMAT_VERSION,
            principals: self.records_by_hash.values().cloned().collect(),
        };
        let serialized = serde_json::to_vec_pretty(&envelope).map_err(|source| {
            relay_error(
                "internal_principal_store",
                "failed to serialize principal store",
                Some(json!({
                    "path": self.path.display().to_string(),
                    "cause": source.to_string(),
                })),
            )
        })?;
        let tmp_path = self.path.with_extension("json.tmp");
        {
            let mut options = fs::OpenOptions::new();
            options
                .create(true)
                .truncate(true)
                .write(true)
                .mode(PRINCIPAL_FILE_MODE);
            let mut file = options
                .open(&tmp_path)
                .map_err(|source| self.io_error("open temp", source))?;
            io::Write::write_all(&mut file, &serialized)
                .map_err(|source| self.io_error("write temp", source))?;
            file.sync_all()
                .map_err(|source| self.io_error("sync temp", source))?;
        }
        fs::rename(&tmp_path, &self.path)
            .map_err(|source| self.io_error("rename store", source))?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(PRINCIPAL_FILE_MODE))
            .map_err(|source| self.io_error("set mode 0600", source))?;
        Ok(())
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

/// Generates a fresh pre-shared key: 32 bytes of OS CSPRNG output encoded as
/// unpadded standard base64.
pub(crate) fn generate_psk() -> String {
    let mut bytes = [0u8; PSK_BYTE_LENGTH];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .expect("OsRng must not fail");
    base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
}

/// Writes `psk` to an operator-supplied absolute `path` with mode 0600.
///
/// The relay refuses to follow symlinks (`O_NOFOLLOW`) and does not create
/// parent directories: the target's parent must already exist. These
/// constraints prevent a `--output` argument from being redirected through a
/// symlink or silently materializing an unexpected directory tree.
pub(crate) fn write_psk_output_file(path: &Path, psk: &str) -> Result<(), RelayError> {
    if !path.is_absolute() {
        return Err(relay_error(
            "validation_invalid_output_path",
            "credential output path must be absolute",
            Some(json!({ "path": path.display().to_string() })),
        ));
    }
    let Some(parent) = path.parent() else {
        return Err(relay_error(
            "validation_invalid_output_path",
            "credential output path has no parent directory",
            Some(json!({ "path": path.display().to_string() })),
        ));
    };
    if !parent.is_dir() {
        return Err(relay_error(
            "validation_invalid_output_path",
            "credential output parent directory does not exist",
            Some(json!({
                "path": path.display().to_string(),
                "parent": parent.display().to_string(),
            })),
        ));
    }
    let mut options = fs::OpenOptions::new();
    options
        .create(true)
        .truncate(true)
        .write(true)
        .mode(PRINCIPAL_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
    let mut file = options.open(path).map_err(|source| {
        relay_error(
            "validation_invalid_output_path",
            "failed to write credential output file",
            Some(json!({
                "path": path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    io::Write::write_all(&mut file, psk.as_bytes()).map_err(|source| {
        relay_error(
            "internal_credential_write",
            "failed to write credential output file",
            Some(json!({
                "path": path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    // Re-assert mode in case a pre-existing file's permissions differed.
    fs::set_permissions(path, fs::Permissions::from_mode(PRINCIPAL_FILE_MODE)).map_err(
        |source| {
            relay_error(
                "internal_credential_write",
                "failed to set credential output file mode",
                Some(json!({
                    "path": path.display().to_string(),
                    "cause": source.to_string(),
                })),
            )
        },
    )?;
    Ok(())
}

/// Returns true when a record's `expires_at` is absent-free but at or before
/// `now`, or is present yet unparseable. A record with no `expires_at` never
/// expires.
fn record_is_expired(record: &PrincipalRecord, now: OffsetDateTime) -> bool {
    match record.expires_at.as_deref() {
        None => false,
        Some(raw) => match OffsetDateTime::parse(raw, &Rfc3339) {
            Ok(expires_at) => expires_at <= now,
            Err(_) => true,
        },
    }
}

/// Returns the lowercase hex SHA-256 digest of `token`.
pub(crate) fn hash_token_sha256(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Splits a `principal_id` of the form `<id>@<namespace>` into its components.
///
/// Returns `None` when the input lacks a single `@` separator or either side
/// is empty.
pub(crate) fn split_principal_id(principal_id: &str) -> Option<(&str, &str)> {
    let (local, namespace) = principal_id.rsplit_once('@')?;
    if local.is_empty() || namespace.is_empty() {
        return None;
    }
    Some((local, namespace))
}

/// Decides whether a host's registered `scope` permits introspecting (or
/// observing revocation of) `target_principal_id`.
///
/// The store records a single scope entry as either a canonical
/// `session_id@bundle_name` identity (exact match) or a bare `bundle_name`
/// (matches every session in that bundle namespace). A `None` scope is
/// fail-closed: no target is in scope.
pub(crate) fn scope_permits(scope: Option<&str>, target_principal_id: &str) -> bool {
    let Some(scope) = scope else {
        return false;
    };
    if scope == target_principal_id {
        return true;
    }
    matches!(
        split_principal_id(target_principal_id),
        Some((_, namespace)) if scope == namespace
    )
}

/// Classifies a `principal_id` by namespace partition.
///
/// `@GLOBAL` → user, `@EXTERNAL` → application, `@RELAY` → peer relay; any
/// other non-empty namespace is treated as a bundle-scoped session principal.
pub(crate) fn classify_principal_id(principal_id: &str) -> Option<PrincipalType> {
    let (_, namespace) = split_principal_id(principal_id)?;
    Some(match namespace {
        "GLOBAL" => PrincipalType::User,
        "EXTERNAL" => PrincipalType::Application,
        "RELAY" => PrincipalType::Relay,
        _ => PrincipalType::Session,
    })
}

/// Sentinel `identity_token` presented by sessions that have no provisioned
/// PSK file. Accepted for session and user principals only when relay-wide
/// credential enforcement is disabled (see `verify_hello_credential`).
pub(crate) const SOCKET_TRUST_TOKEN: &str = "socket-trust";

/// Introspection rights recorded on an application principal's connection at
/// Hello, so request dispatch can gate `IdentityIntrospect` on them.
///
/// Present only for `Application` principals (which always present a recognized
/// credential). The `scope` is the principal store record's `scope` field, set
/// at `new peer` registration for `@EXTERNAL` principals; `None` means the
/// principal was registered without a scope bound, which the dispatch gate
/// treats as fail-closed (no target is in scope).
#[derive(Clone, Debug)]
pub(crate) struct IdentityIntrospectRights {
    pub(crate) scope: Option<String>,
}

/// Result of verifying a Hello credential against the principal store.
pub(crate) struct VerifiedIdentity {
    pub(crate) principal_type: PrincipalType,
    /// True when a recognized store credential backed the identity; false for
    /// accepted `"socket-trust"` connections, which create no store entry.
    /// Drives sender-attribution (`authenticated_identity`) and distinguishes
    /// store-backed from socket-trust connections on the Hello path.
    pub(crate) store_backed: bool,
    /// Introspection rights for an `Application` principal, carrying its
    /// registered scope; `None` for every other principal type. Recorded on the
    /// connection context so request dispatch can gate `IdentityIntrospect`
    /// (task 2.5).
    pub(crate) introspect_rights: Option<IdentityIntrospectRights>,
}

/// Verifies a Hello `principal_id` + `identity_token` against the principal
/// store and the relay-wide enforcement policy.
///
/// A recognized token must be registered to the claimed `principal_id`
/// (credential-to-identity binding). The `"socket-trust"` sentinel is accepted
/// for session and user principals only when enforcement is disabled;
/// application and relay principals always require a recognized credential.
/// Unrecognized non-sentinel tokens are rejected fail-closed regardless of
/// enforcement.
///
/// Expiry is detected here rather than by pruning the store before lookup: a
/// recognized credential whose record has expired is rejected with the distinct
/// `runtime_identity_expired` error (carrying `now`), so an expiring session is
/// told its credential lapsed rather than receiving the generic
/// `validation_unrecognized_credential`.
pub(crate) fn verify_hello_credential(
    principal_id: &str,
    identity_token: &str,
    store: &PrincipalStore,
    require_session_credentials: bool,
    now: OffsetDateTime,
) -> Result<VerifiedIdentity, RelayError> {
    let Some(claimed_type) = classify_principal_id(principal_id) else {
        return Err(relay_error(
            "validation_invalid_principal_id",
            "hello principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    if identity_token == SOCKET_TRUST_TOKEN {
        return verify_socket_trust(principal_id, claimed_type, require_session_credentials);
    }
    let hash = hash_token_sha256(identity_token);
    let Some(record) = store.find_by_credential_hash(&hash) else {
        return Err(relay_error(
            "validation_unrecognized_credential",
            "hello identity_token did not match any registered principal",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    if record.principal_id != principal_id {
        return Err(relay_error(
            "validation_identity_binding_mismatch",
            "presented credential is registered to a different principal_id",
            Some(json!({
                "principal_id": principal_id,
                "registered_principal_id": record.principal_id,
            })),
        ));
    }
    if record.is_expired(now) {
        return Err(relay_error(
            "runtime_identity_expired",
            "identity credential has expired; re-register or rotate the credential",
            Some(json!({
                "principal_id": principal_id,
                "expires_at": record.expires_at,
            })),
        ));
    }
    let introspect_rights =
        (record.principal_type == PrincipalType::Application).then(|| IdentityIntrospectRights {
            scope: record.scope.clone(),
        });
    Ok(VerifiedIdentity {
        principal_type: record.principal_type,
        store_backed: true,
        introspect_rights,
    })
}

fn verify_socket_trust(
    principal_id: &str,
    claimed_type: PrincipalType,
    require_session_credentials: bool,
) -> Result<VerifiedIdentity, RelayError> {
    match claimed_type {
        PrincipalType::Application | PrincipalType::Relay => Err(relay_error(
            "validation_credential_required",
            "application and relay principals require a registered credential",
            Some(json!({ "principal_id": principal_id })),
        )),
        PrincipalType::Session | PrincipalType::User => {
            if require_session_credentials {
                return Err(relay_error(
                    "validation_credential_required",
                    "relay requires session credentials; socket-trust is not accepted",
                    Some(json!({ "principal_id": principal_id })),
                ));
            }
            Ok(VerifiedIdentity {
                principal_type: claimed_type,
                store_backed: false,
                // Socket-trust is accepted only for session and user
                // principals, never `Application`, so it grants no introspect
                // rights.
                introspect_rights: None,
            })
        }
    }
}
