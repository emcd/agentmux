use std::{
    collections::HashMap,
    fs, io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use base64::Engine;
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::runtime::paths::{is_valid_bundle_name, session_identity_psk_path};

use super::{CredentialDestination, GLOBAL_SESSION_SUFFIX, RelayError, relay_error};

const PSK_BYTE_LENGTH: usize = 32;
const PRINCIPAL_FILE_MODE: u32 = 0o600;
const PRINCIPAL_STORE_FORMAT_VERSION: u32 = 1;
/// Upper bound on a `config`-destination session-id component, mirroring
/// `configuration::SESSION_ID_LENGTH_MAX` (kept in sync manually since that
/// constant is crate-private to the configuration module).
const CONFIG_SESSION_ID_LENGTH_MAX: usize = 31;

/// Process-unique nonce source for temp-file names, so a staged store or
/// credential temp never collides with a concurrent or stale artifact.
static TEMP_FILE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Builds a per-attempt-unique sibling temp path for `final_path`. Combining the
/// pid with a monotonic nonce keeps two concurrent writers (and any stale
/// crash-left artifact) from ever selecting the same temp name, so a
/// `create_new` open can safely refuse a pre-existing file.
fn unique_temp_path(final_path: &Path, tag: &str) -> PathBuf {
    let nonce = TEMP_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut name = final_path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{pid}.{nonce}.{tag}.tmp"));
    final_path.with_file_name(name)
}

/// Returns the canonical `session@namespace` identity for a session id.
///
/// Global-user identities already carry the `@GLOBAL` suffix and are their own
/// canonical form; bundle-local identities are qualified with the namespace.
pub(super) fn canonical_session_id(session_id: &str, namespace: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        session_id.to_string()
    } else {
        format!("{session_id}@{namespace}")
    }
}

/// Returns the bundle-local session id for a possibly-canonical identity.
///
/// Strips a trailing `@{namespace}` qualifier so internal lookups match
/// configured member ids; global-user (`@GLOBAL`) identities and already-bare
/// ids are returned unchanged.
pub(super) fn bare_session_id(session_id: &str, namespace: &str) -> String {
    if session_id.ends_with(GLOBAL_SESSION_SUFFIX) {
        return session_id.to_string();
    }
    let qualifier = format!("@{namespace}");
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
        // Stage into a per-attempt-unique sibling (`create_new`, so a stale or
        // concurrent temp can never be reused) and enforce mode BEFORE the
        // rename, so the atomic rename is the single, last fallible step. A
        // failure after publication (e.g. a post-rename chmod) would otherwise
        // report an error while the new store is already durable, defeating the
        // handlers' rollback.
        let tmp_path = unique_temp_path(&self.path, "store");
        {
            let mut options = fs::OpenOptions::new();
            options
                .create_new(true)
                .write(true)
                .mode(PRINCIPAL_FILE_MODE);
            let mut file = options
                .open(&tmp_path)
                .map_err(|source| self.io_error("open temp", source))?;
            if let Err(source) =
                io::Write::write_all(&mut file, &serialized).and_then(|()| file.sync_all())
            {
                let _ = fs::remove_file(&tmp_path);
                return Err(self.io_error("write temp", source));
            }
        }
        if let Err(source) =
            fs::set_permissions(&tmp_path, fs::Permissions::from_mode(PRINCIPAL_FILE_MODE))
        {
            let _ = fs::remove_file(&tmp_path);
            return Err(self.io_error("set mode 0600", source));
        }
        // Commit point: nothing fallible runs after this rename.
        fs::rename(&tmp_path, &self.path).map_err(|source| {
            let _ = fs::remove_file(&tmp_path);
            self.io_error("rename store", source)
        })?;
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

/// A validated credential destination, resolved to a concrete sink before any
/// principal-store mutation so a rejected destination registers or rotates
/// nothing.
pub(crate) enum StagedCredentialSink {
    /// Return the PSK in the response.
    Response,
    /// Write the PSK to `path`. `create_parents` is set only for relay-owned
    /// `config` paths; caller-named `path` sinks require a pre-existing parent.
    File { path: PathBuf, create_parents: bool },
}

/// A credential written to a temp sibling, awaiting the atomic rename that
/// publishes it. The write lands before the store commit; the rename runs after
/// it, so a store failure discards the staged file and leaves the prior
/// credential (if any) intact.
pub(crate) struct PendingCredentialWrite {
    tmp_path: PathBuf,
    final_path: PathBuf,
}

impl PendingCredentialWrite {
    /// Atomically publishes the staged credential and returns the written path.
    ///
    /// The temp file's mode is enforced at staging time (before this call), so
    /// the rename is the single, last fallible step — no post-publication work
    /// can report an error after the credential is already in place.
    pub(crate) fn commit(self) -> Result<String, RelayError> {
        fs::rename(&self.tmp_path, &self.final_path).map_err(|source| {
            let _ = fs::remove_file(&self.tmp_path);
            relay_error(
                "internal_credential_write",
                "failed to finalize credential file",
                Some(json!({
                    "path": self.final_path.display().to_string(),
                    "cause": source.to_string(),
                })),
            )
        })?;
        Ok(self.final_path.display().to_string())
    }

    /// Discards the staged temp file without publishing it.
    pub(crate) fn abort(self) {
        let _ = fs::remove_file(&self.tmp_path);
    }
}

/// Validates `destination` and resolves it to a concrete sink without writing
/// anything or touching the store. `principal_type` and `principal_id` gate and
/// derive the `config` credential path.
pub(crate) fn stage_credential_sink(
    destination: &CredentialDestination,
    principal_type: PrincipalType,
    principal_id: &str,
    state_root: &Path,
) -> Result<StagedCredentialSink, RelayError> {
    match destination {
        CredentialDestination::Response => Ok(StagedCredentialSink::Response),
        CredentialDestination::Path { path } => {
            let path = validate_output_path(Path::new(path))?;
            Ok(StagedCredentialSink::File {
                path,
                create_parents: false,
            })
        }
        CredentialDestination::Config => {
            let path = resolve_config_credential_path(principal_type, principal_id, state_root)?;
            Ok(StagedCredentialSink::File {
                path,
                create_parents: true,
            })
        }
    }
}

/// Writes `psk` to the sink's temp sibling (0600, `O_NOFOLLOW`, fsync), creating
/// parent directories only for relay-owned config paths. Returns `None` for the
/// response sink (nothing to publish).
pub(crate) fn write_pending_credential(
    sink: &StagedCredentialSink,
    psk: &str,
) -> Result<Option<PendingCredentialWrite>, RelayError> {
    let StagedCredentialSink::File {
        path,
        create_parents,
    } = sink
    else {
        return Ok(None);
    };
    if *create_parents && let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            relay_error(
                "internal_credential_write",
                "failed to create credential directory",
                Some(json!({
                    "path": parent.display().to_string(),
                    "cause": source.to_string(),
                })),
            )
        })?;
        let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
    }
    // `create_new` (O_EXCL) guarantees we materialize a fresh 0600 file rather
    // than truncating a stale sibling whose looser permissions would briefly
    // hold the raw PSK; `O_NOFOLLOW` refuses a symlinked temp.
    let tmp_path = unique_temp_path(path, "cred");
    let mut options = fs::OpenOptions::new();
    options
        .create_new(true)
        .write(true)
        .mode(PRINCIPAL_FILE_MODE)
        .custom_flags(libc::O_NOFOLLOW);
    let file = options.open(&tmp_path).map_err(|source| {
        relay_error(
            "internal_credential_write",
            "failed to open credential temp file",
            Some(json!({
                "path": tmp_path.display().to_string(),
                "cause": source.to_string(),
            })),
        )
    })?;
    // Enforce exactly 0600 before the secret is written, then publish via the
    // rename commit point, so no fallible permission step follows publication.
    if let Err(source) =
        fs::set_permissions(&tmp_path, fs::Permissions::from_mode(PRINCIPAL_FILE_MODE))
    {
        let _ = fs::remove_file(&tmp_path);
        return Err(relay_error(
            "internal_credential_write",
            "failed to set credential temp file mode",
            Some(json!({
                "path": tmp_path.display().to_string(),
                "cause": source.to_string(),
            })),
        ));
    }
    let mut file = file;
    let write_result =
        io::Write::write_all(&mut file, psk.as_bytes()).and_then(|()| file.sync_all());
    if let Err(source) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(relay_error(
            "internal_credential_write",
            "failed to write credential temp file",
            Some(json!({
                "path": tmp_path.display().to_string(),
                "cause": source.to_string(),
            })),
        ));
    }
    Ok(Some(PendingCredentialWrite {
        tmp_path,
        final_path: path.clone(),
    }))
}

/// Validates a caller-named `path` sink: it must be absolute, have a file name,
/// have an existing parent directory, and not be a symlinked target. Parent
/// directories are never created for a caller-named path.
fn validate_output_path(path: &Path) -> Result<PathBuf, RelayError> {
    if !path.is_absolute() {
        return Err(invalid_output_path(
            path,
            "credential output path must be absolute",
        ));
    }
    if path.file_name().is_none() {
        return Err(invalid_output_path(
            path,
            "credential output path has no file name",
        ));
    }
    let Some(parent) = path.parent() else {
        return Err(invalid_output_path(
            path,
            "credential output path has no parent directory",
        ));
    };
    if !parent.is_dir() {
        return Err(invalid_output_path(
            path,
            "credential output parent directory does not exist",
        ));
    }
    // Preserve the no-follow contract: writing through a symlinked target is
    // refused. The temp+rename publish never opens the final path directly, so
    // this lstat check is the enforcement point for the caller-named sink.
    if let Ok(metadata) = fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(invalid_output_path(
            path,
            "credential output path is a symlink",
        ));
    }
    Ok(path.to_path_buf())
}

/// Derives the relay-owned canonical credential path for a `config` sink.
///
/// Only session principals have a relay-owned credential location; peer-relay,
/// user, and application principals do not, so `config` is rejected for them.
/// The `<id>@<namespace>` components become filesystem path segments, so each is
/// checked against a safe-segment grammar to keep a crafted principal id from
/// escaping the state root.
fn resolve_config_credential_path(
    principal_type: PrincipalType,
    principal_id: &str,
    state_root: &Path,
) -> Result<PathBuf, RelayError> {
    if principal_type != PrincipalType::Session {
        return Err(relay_error(
            "validation_config_destination_unsupported",
            "config credential destination is only supported for session principals",
            Some(json!({
                "principal_id": principal_id,
                "principal_type": principal_type.as_str(),
            })),
        ));
    }
    let Some((session_id, bundle_name)) = split_principal_id(principal_id) else {
        return Err(relay_error(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    // Enforce the real identity grammar, not merely path-safety: a config
    // destination must name an identity a configured session could actually own.
    // The session-id grammar (leading ASCII alpha, then alphanumeric/`-`/`_`)
    // and the namespace grammar both exclude `/`, `.`, and `..`, so this also
    // subsumes traversal rejection.
    if !is_valid_session_component(session_id) || !is_valid_namespace_component(bundle_name) {
        return Err(relay_error(
            "validation_invalid_principal_id",
            "principal id components are not a valid session identity for a config destination",
            Some(json!({ "principal_id": principal_id })),
        ));
    }
    Ok(session_identity_psk_path(
        state_root,
        bundle_name,
        session_id,
    ))
}

/// True when `session_id` matches the configured session-id grammar
/// (`configuration::fields::validate_session_id`): non-empty, first character
/// ASCII alphabetic, remaining characters ASCII alphanumeric / `-` / `_`, within
/// the length bound. Rejects ids no configured session could own (e.g. `1worker`,
/// `worker.name`) and, by construction, `.` / `..` / path separators.
fn is_valid_session_component(session_id: &str) -> bool {
    let mut characters = session_id.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphabetic()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        && session_id.len() <= CONFIG_SESSION_ID_LENGTH_MAX
}

/// True when `namespace` is a valid bundle name usable as a path segment: it
/// satisfies the canonical bundle-name grammar (which permits `.`, so real
/// dotted bundle names like `team.one` are accepted) but is not a traversal-only
/// `.` / `..` segment. The grammar already excludes `/`, so this cannot escape
/// the state root.
fn is_valid_namespace_component(namespace: &str) -> bool {
    is_valid_bundle_name(namespace) && namespace != "." && namespace != ".."
}

fn invalid_output_path(path: &Path, message: &str) -> RelayError {
    relay_error(
        "validation_invalid_output_path",
        message,
        Some(json!({ "path": path.display().to_string() })),
    )
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
    /// Cross-relay ingress scope for a `Relay` (peer) principal: the store
    /// record's registered `scope` (set via `new peer <id>@RELAY --scope`),
    /// bounding which targets a forwarded `Send`/`Raww` from this peer may reach.
    /// `None` for every other principal type, and `None` for a peer registered
    /// without a scope (which the ingress gate treats as fail-closed). Kept
    /// separate from `introspect_rights` so a peer relay gains only delivery
    /// ingress, not the application-only identity snapshot or revocation fan-out.
    pub(crate) ingress_scope: Option<String>,
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
    let ingress_scope = (record.principal_type == PrincipalType::Relay)
        .then(|| record.scope.clone())
        .flatten();
    Ok(VerifiedIdentity {
        principal_type: record.principal_type,
        store_backed: true,
        introspect_rights,
        ingress_scope,
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
                // principals, never `Application` or `Relay`, so it grants no
                // introspect rights and no cross-relay ingress scope.
                introspect_rights: None,
                ingress_scope: None,
            })
        }
    }
}
