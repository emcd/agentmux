//! Public embeddable relay runtime: handle, identity descriptors, verified
//! principal context, provisioning, and introspection.
//!
//! This module is the public embedding boundary for relay operations whose
//! public dispatch handlers validate verified context through
//! [`check_context`]. It defines the runtime handle that owns per-instance
//! state, the caller-supplied identity descriptor vocabulary (including
//! `direct_psk`), the opaque relay-verified principal context with its
//! credential-binding epoch, and the trusted provisioning/introspection
//! boundary.
//!
//! Authority model: whoever holds the [`RelayRuntime`] handle is the trusted
//! provisioning path — provisioning and introspection take the handle, never
//! a verified principal. The socket path keeps its policy-gated handlers
//! unchanged; those remain the untrusted-surface path. Dispatch handlers
//! take `&VerifiedPrincipal` and validate it through [`check_context`],
//! which is the single boundary where the binding epoch, instance binding,
//! and shutdown gate are enforced.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use serde_json::json;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::configuration::ConfigurationRoots;
use crate::relay::DeliveryConfiguration;
use crate::runtime::inscriptions::emit_inscription;
use crate::runtime::paths::{
    RelayRuntimePaths, ensure_relay_runtime_directory,
};

use super::DeliveryContext;
use super::RelayError;
use super::configuration::load_relay_runtime_configuration;
use super::contract::CredentialDestination;
use super::handlers::{
    build_config_snippet, credential_rollback_failed, is_credential_uncertain,
    is_durability_uncertain, revoke_superseded_credential,
};
use super::identity::{
    PrincipalRecord, PrincipalStore, PrincipalType, classify_principal_id,
    generate_psk, hash_token_sha256, lock_store_transaction,
    split_principal_id, stage_credential_sink, write_pending_credential,
};
use super::peer_scope::parse_peer_scope;
use super::runtime_domains::{
    canonical_domain_key, claim_domain_slot, release_domain_slot,
};
use super::stream::{
    RelayStreamEvent, notify_trusted_hosts_of_revocation,
    revoke_streams_for_identity,
};

/// An embedded relay runtime handle: one live relay instance.
///
/// Owns the configuration roots, the state root locating the identity store,
/// the per-instance delivery state, and the instance binding that verified
/// principal contexts are minted against.
///
/// One store root hosts one runtime per process: `init` refuses a second
/// live runtime on the same root, and the socket identity-admin dispatch
/// routes revocation and notification effects through the owning runtime's
/// delivery state rather than the serving connection's own. Two handles on
/// separate roots stay isolated by construction: separate roots, separate
/// delivery state, separate instance ids. Sharing one store root across
/// processes is operator error: the registry is in-process only and cannot
/// serialize against another process's writers.
pub struct RelayRuntime {
    configuration_roots: ConfigurationRoots,
    state_root: PathBuf,
    delivery: Arc<DeliveryContext>,
    instance_id: Uuid,
    require_session_credentials: bool,
    /// Canonical registry key of the hosted store root, for release on drop.
    domain_key: PathBuf,
    /// Weak handle to this runtime's delivery state, for release on drop.
    domain_weak: Weak<DeliveryContext>,
}

impl Drop for RelayRuntime {
    /// Releases this runtime's store-root ownership, but only when the
    /// registry still points at this runtime: a stale entry is left for the
    /// owner that replaced it.
    fn drop(&mut self) {
        release_domain_slot(&self.domain_key, &self.domain_weak);
    }
}

impl RelayRuntime {
    /// Initializes a relay runtime against caller-supplied roots.
    ///
    /// The configuration root supplies relay/policy configuration and the
    /// state root locates the principal store and receives Agentmux-owned
    /// runtime artifacts beneath it. Neither root may assume a fixed parent
    /// layout: they are used exactly as supplied, and only well-known
    /// leaf names are appended beneath them.
    ///
    /// The resolved `[delivery]` table is taken explicitly rather than read
    /// from `relay.toml`, so the embedding host owns it the way the
    /// standalone host owns the table it resolves before constructing its
    /// handle. `require_session_credentials` still follows the relay
    /// configuration ladder (CLI override > environment > `relay.toml` >
    /// documented default) for policy parity with the standalone relay.
    ///
    /// # Errors
    ///
    /// Returns the loader's [`RelayError`] when `relay.toml` is malformed,
    /// an internal failure when the state-root layout cannot be ensured,
    /// or `validation_runtime_already_hosted` when another live runtime in
    /// this process already hosts the same state root. Cross-process
    /// sharing of one store root is operator error and is not detected.
    pub fn init(
        configuration_root: PathBuf,
        state_root: PathBuf,
        delivery: DeliveryConfiguration,
    ) -> Result<Self, RelayError> {
        let configuration_roots =
            ConfigurationRoots::single(configuration_root);
        let runtime_configuration = load_relay_runtime_configuration(
            &configuration_roots,
            None,
            None,
        )?;
        let relay_paths = RelayRuntimePaths::resolve(state_root.as_path());
        ensure_relay_runtime_directory(&relay_paths).map_err(|source| {
            crate::relay::relay_error(
                "internal_unexpected_failure",
                "failed to ensure relay state-root layout",
                Some(json!({ "cause": source.to_string() })),
            )
        })?;
        // Claim the single-revocation-domain ownership after the layout
        // exists, so the canonical key resolves: a second live runtime on
        // the same root is refused while the first is alive.
        let owned_delivery = Arc::new(DeliveryContext::new(delivery));
        let domain_key = canonical_domain_key(state_root.as_path());
        let domain_weak = claim_domain_slot(&domain_key, &owned_delivery)?;
        Ok(Self {
            configuration_roots,
            state_root,
            delivery: owned_delivery,
            instance_id: Uuid::new_v4(),
            require_session_credentials: runtime_configuration
                .require_session_credentials,
            domain_key,
            domain_weak,
        })
    }

    /// The configuration roots this runtime loads relay and policy
    /// configuration from.
    #[must_use]
    pub fn configuration_roots(&self) -> &ConfigurationRoots {
        &self.configuration_roots
    }

    /// The state root locating this runtime's principal store and
    /// Agentmux-owned runtime artifacts.
    #[must_use]
    pub fn state_root(&self) -> &Path {
        self.state_root.as_path()
    }

    /// This runtime's per-instance delivery state.
    #[must_use]
    pub fn delivery_context(&self) -> &Arc<DeliveryContext> {
        &self.delivery
    }

    /// Whether relay policy requires store-backed credentials.
    #[must_use]
    pub fn require_session_credentials(&self) -> bool {
        self.require_session_credentials
    }

    /// Begins controlled shutdown for this runtime.
    ///
    /// Dispatch validated through [`check_context`] is refused from here on.
    /// Worker loops keep their process-global kill-switch reads; the gate
    /// signalled here is per-instance.
    pub fn begin_shutdown(&self) {
        self.delivery.shutdown_gate().signal_shutdown();
    }
}

/// A caller-supplied identity descriptor: who the caller claims to be, what
/// credential material it presents, and optional routing defaults.
///
/// Descriptors are input to verification or provisioning only. They are never
/// accepted as verified authorization context: [`check_context`] takes only
/// [`VerifiedPrincipal`], and no API converts a descriptor into dispatch
/// authority without a successful credential verification.
#[derive(Clone, Debug)]
pub struct IdentityDescriptor {
    /// Claimed `principal_id` in `<id>@<namespace>` form.
    pub principal_id: String,
    /// Presented credential material.
    pub credential: CredentialSource,
    /// Optional operation-bundle routing default. Defines operation-bundle
    /// defaults only, never identity: dispatch still resolves the principal
    /// home from verification, not from this field.
    pub operation_bundle: Option<String>,
    /// Optional descriptive metadata. Never treated as authorization
    /// authority.
    pub metadata: HashMap<String, String>,
}

impl IdentityDescriptor {
    /// Builds a descriptor carrying the given credential source.
    #[must_use]
    pub fn new(principal_id: String, credential: CredentialSource) -> Self {
        Self {
            principal_id,
            credential,
            operation_bundle: None,
            metadata: HashMap::new(),
        }
    }
}

/// The credential source a descriptor presents.
///
/// The vocabulary carries host-held credential material. `socket-trust` is
/// deliberately absent: it is a transport-surface fallback, and in-process
/// verification always requires presented material.
///
/// `Debug` redacts the material: the variant is shown, the secret is not.
#[derive(Clone)]
pub enum CredentialSource {
    /// Raw PSK material held in memory by the embedding host: the
    /// credential file contents for an Agentmux-managed principal, or the
    /// one-time secret returned by provisioning for a dynamic agent.
    DirectPsk(String),
}

impl std::fmt::Debug for CredentialSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectPsk(_) => {
                f.debug_tuple("DirectPsk").field(&"***").finish()
            },
        }
    }
}

/// Relay-verified principal context: dispatch authority frozen to the
/// credential/principal binding epoch.
///
/// Opaque and non-caller-constructible: the only producers are
/// [`verify_identity`] and the authorized provisioning paths. It carries the
/// principal home as an explicit field set once at verification time, the
/// credential hash of the binding it was minted under, and the issuing
/// runtime's instance id. Rotation, revocation, or deletion invalidates
/// outstanding contexts; recreating a principal starts a new epoch and does
/// not resurrect them. The binding epoch is not a delivery generation:
/// delivery progress never invalidates context and never extends a revoked
/// binding.
#[allow(dead_code)] // Read through the dispatch boundary, which takes verified context.
#[derive(Clone, Debug)]
pub struct VerifiedPrincipal {
    principal_id: String,
    home_namespace: String,
    epoch_hash: String,
    instance_id: Uuid,
    principal_type: PrincipalType,
}

// Read through the dispatch boundary, which takes verified context.
#[allow(dead_code)]
impl VerifiedPrincipal {
    /// The verified `principal_id`.
    #[must_use]
    pub(crate) fn principal_id(&self) -> &str {
        self.principal_id.as_str()
    }

    /// The principal home frozen at verification time.
    #[must_use]
    pub(crate) fn home_namespace(&self) -> &str {
        self.home_namespace.as_str()
    }

    /// The credential hash of the binding this context was minted under.
    #[must_use]
    pub(crate) fn epoch_hash(&self) -> &str {
        self.epoch_hash.as_str()
    }

    /// The issuing runtime's instance id.
    #[must_use]
    pub(crate) fn instance_id(&self) -> Uuid {
        self.instance_id
    }

    /// The principal type recorded at verification time.
    #[must_use]
    pub(crate) fn principal_type(&self) -> PrincipalType {
        self.principal_type
    }
}

/// Verifies a caller-supplied identity descriptor against this runtime's
/// identity store and mints relay-verified principal context.
///
/// The presented credential is hashed and looked up in the principal store;
/// the record's `principal_id` must equal the claimed one (binding check)
/// and the record must be unexpired. Home is frozen from the record's own
/// principal id at mint time, never re-derived at dispatch.
///
/// In-process verification always requires presented credential material:
/// there is no `socket-trust` fallback here. That fallback belongs to the
/// socket transport's Hello path, which also consults
/// `require_session_credentials`; the descriptor vocabulary carries no
/// trust sentinel.
///
/// # Errors
///
/// Returns `validation_invalid_principal_id` for a malformed id,
/// `validation_unrecognized_credential` for an unknown hash,
/// `validation_identity_binding_mismatch` when the hash names a different
/// principal, and `runtime_identity_expired` for an expired record.
pub fn verify_identity(
    runtime: &RelayRuntime,
    descriptor: &IdentityDescriptor,
) -> Result<VerifiedPrincipal, RelayError> {
    let CredentialSource::DirectPsk(psk) = &descriptor.credential;
    let presented_hash = hash_token_sha256(psk.as_str());
    let store = PrincipalStore::load(runtime.state_root.as_path())?;
    let Some(record) = store.find_by_credential_hash(presented_hash.as_str())
    else {
        return Err(crate::relay::relay_error(
            "validation_unrecognized_credential",
            "presented credential matches no registered principal",
            None,
        ));
    };
    if record.principal_id != descriptor.principal_id {
        return Err(crate::relay::relay_error(
            "validation_identity_binding_mismatch",
            "presented credential names a different principal than claimed",
            Some(json!({ "principal_id": descriptor.principal_id })),
        ));
    }
    if record.is_expired(OffsetDateTime::now_utc()) {
        return Err(crate::relay::relay_error(
            "runtime_identity_expired",
            "principal credential has expired",
            Some(json!({ "principal_id": descriptor.principal_id })),
        ));
    }
    let principal_type = classify_principal_id(
        descriptor.principal_id.as_str(),
    )
    .ok_or_else(|| {
        crate::relay::relay_error(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form",
            Some(json!({
                "principal_id": descriptor.principal_id
            })),
        )
    })?;
    let Some((_, namespace)) =
        split_principal_id(descriptor.principal_id.as_str())
    else {
        return Err(crate::relay::relay_error(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": descriptor.principal_id })),
        ));
    };
    Ok(VerifiedPrincipal {
        principal_id: record.principal_id.clone(),
        home_namespace: namespace.to_string(),
        epoch_hash: record.credential_hash.clone(),
        instance_id: runtime.instance_id,
        principal_type,
    })
}

/// Validates verified principal context against this runtime: the single
/// boundary where binding epoch, instance binding, and the shutdown gate are
/// enforced.
///
/// Dispatch handlers call this before routing or authorization. It rejects a
/// context minted by another runtime instance, a runtime in controlled
/// shutdown, and a context whose binding no longer matches the identity
/// store (rotated, revoked, deleted, or recreated under the same id, which
/// starts a new epoch and does not resurrect the old context).
///
/// # Errors
///
/// Returns `validation_foreign_relay_instance` for a context minted by a
/// different runtime, `runtime_shutting_down` once shutdown begins,
/// `validation_stale_verified_context` when the binding changed or vanished,
/// and `runtime_identity_expired` when the record has since expired.
pub fn check_context(
    runtime: &RelayRuntime,
    context: &VerifiedPrincipal,
) -> Result<(), RelayError> {
    if context.instance_id != runtime.instance_id {
        return Err(crate::relay::relay_error(
            "validation_foreign_relay_instance",
            "verified context was minted by a different runtime instance",
            None,
        ));
    }
    if runtime.delivery.is_shutting_down() {
        return Err(crate::relay::relay_error(
            "runtime_shutting_down",
            "relay handle is shutting down",
            None,
        ));
    }
    let store = PrincipalStore::load(runtime.state_root.as_path())?;
    let Some(record) =
        store.find_by_principal_id(context.principal_id.as_str())
    else {
        return Err(crate::relay::relay_error(
            "validation_stale_verified_context",
            "principal binding no longer exists under this identity store",
            Some(json!({ "principal_id": context.principal_id })),
        ));
    };
    if record.credential_hash != context.epoch_hash {
        return Err(crate::relay::relay_error(
            "validation_stale_verified_context",
            "principal credential binding changed since this context was minted",
            Some(json!({ "principal_id": context.principal_id })),
        ));
    }
    if record.is_expired(OffsetDateTime::now_utc()) {
        return Err(crate::relay::relay_error(
            "runtime_identity_expired",
            "principal credential has expired",
            Some(json!({ "principal_id": context.principal_id })),
        ));
    }
    Ok(())
}

/// Parameters for provisioning a principal: creation or record update through
/// the trusted provisioning path (the runtime handle itself).
///
/// Provisioning mints or rotates bindings only through these public
/// functions. Read-only introspection never mints dispatch authority, and
/// presenting introspected data as verified context is rejected by
/// construction: there is no API that builds a [`VerifiedPrincipal`] from
/// anything but a successful credential verification.
#[derive(Clone, Debug)]
pub struct ProvisionParams {
    /// `principal_id` in `<id>@<namespace>` form.
    pub principal_id: String,
    /// Ingress scope for `Application`/`Relay` principals; canonicalized
    /// for relays. Ignored authority-wise for sessions and users.
    pub scope: Option<String>,
    /// Where the fresh credential material goes. `Response` returns the raw
    /// PSK once, in the provisioning response only; file sinks omit it.
    pub destination: CredentialDestination,
    /// Descriptive metadata persisted with the record. Never treated as
    /// authorization authority.
    pub metadata: HashMap<String, String>,
}

impl ProvisionParams {
    /// Builds creation parameters returning the raw credential in-response.
    #[must_use]
    pub fn new(principal_id: String) -> Self {
        Self {
            principal_id,
            scope: None,
            destination: CredentialDestination::default(),
            metadata: HashMap::new(),
        }
    }
}

/// Credential material from provisioning or rotation.
///
/// Raw PSK material appears here, and only here: the identity store keeps
/// hashes alone, and introspection never returns credential material.
///
/// `Debug` redacts the raw material: presence is shown, the secret is not.
#[derive(Clone)]
pub struct ProvisionedCredential {
    /// The provisioned `principal_id`.
    pub principal_id: String,
    /// Raw PSK, present only when the destination is `Response`.
    pub psk: Option<String>,
    /// Credential file written, present only for file destinations.
    pub written_path: Option<PathBuf>,
    /// Configuration snippet describing where a session principal's
    /// credential lives.
    pub config_snippet: String,
}

impl std::fmt::Debug for ProvisionedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionedCredential")
            .field("principal_id", &self.principal_id)
            .field("psk", &self.psk.as_ref().map(|_| "***"))
            .field("written_path", &self.written_path)
            .field("config_snippet", &self.config_snippet)
            .finish()
    }
}

/// Provisions a principal: generates a PSK, stores its hash, and either
/// returns the raw PSK once or writes it to the requested credential
/// destination.
///
/// This is the trusted provisioning path the runtime handle authorizes by
/// possession: whoever holds the handle may provision. The socket path keeps
/// its policy-gated admin handlers unchanged.
///
/// # Errors
///
/// Returns `validation_invalid_principal_id` for a malformed id,
/// `validation_principal_exists` for a duplicate, destination validation
/// failures for bad sinks, and durability errors carrying the effective
/// outcome when the store rename already published.
pub fn provision_principal(
    runtime: &RelayRuntime,
    params: ProvisionParams,
) -> Result<ProvisionedCredential, RelayError> {
    let principal_type = classify_principal_id(params.principal_id.as_str())
        .ok_or_else(|| {
        crate::relay::relay_error(
            "validation_invalid_principal_id",
            "principal_id is not in <id>@<namespace> form",
            Some(json!({ "principal_id": params.principal_id })),
        )
    })?;
    let canonical_scope = if principal_type == PrincipalType::Relay {
        parse_peer_scope(params.scope.as_deref())?
    } else {
        params.scope.clone()
    };
    let store_lock = lock_store_transaction(&runtime.state_root);
    let _store_guard = store_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut store = PrincipalStore::load(runtime.state_root.as_path())?;
    store.prune_expired(OffsetDateTime::now_utc());
    if store
        .find_by_principal_id(params.principal_id.as_str())
        .is_some()
    {
        return Err(crate::relay::relay_error(
            "validation_principal_exists",
            "principal_id is already registered; rotate with change psk instead",
            Some(json!({ "principal_id": params.principal_id })),
        ));
    }
    let sink = stage_credential_sink(
        &params.destination,
        principal_type,
        params.principal_id.as_str(),
        runtime.state_root.as_path(),
    )?;
    let psk = generate_psk();
    let credential_hash = hash_token_sha256(psk.as_str());
    let pending = write_pending_credential(
        &sink,
        psk.as_str(),
        runtime.state_root.as_path(),
    )?;
    store.insert(PrincipalRecord {
        principal_id: params.principal_id.clone(),
        principal_type,
        credential_hash,
        scope: canonical_scope,
        expires_at: None,
        metadata: params.metadata.clone(),
    });
    if let Err(error) = store.persist() {
        if !is_durability_uncertain(&error) {
            if let Some(pending) = pending {
                pending.abort();
            }
            return Err(error);
        }
        store.remove_by_principal_id(params.principal_id.as_str());
        match store.persist() {
            Ok(()) => {
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            },
            Err(rollback) if is_durability_uncertain(&rollback) => {
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            },
            Err(rollback) => {
                if let Some(pending) = pending {
                    let _ = pending.commit();
                }
                return Err(credential_rollback_failed(error, rollback));
            },
        }
    }
    let config_snippet = build_config_snippet(
        principal_type,
        params.principal_id.as_str(),
        runtime.state_root.as_path(),
    );
    let (returned_psk, written_path) = match pending {
        None => (Some(psk), None),
        Some(pending) => match pending.commit() {
            Ok(path) => (None, Some(PathBuf::from(path))),
            Err(error) if is_credential_uncertain(&error) => {
                return Err(error);
            },
            Err(error) => {
                store.remove_by_principal_id(params.principal_id.as_str());
                if let Err(rollback) = store.persist() {
                    return Err(credential_rollback_failed(error, rollback));
                }
                return Err(error);
            },
        },
    };
    Ok(ProvisionedCredential {
        principal_id: params.principal_id,
        psk: returned_psk,
        written_path,
        config_snippet,
    })
}

/// Rotates the PSK for an existing principal, preserving its type, scope,
/// expiry, and metadata.
///
/// The store update alone keeps an already-authenticated session alive until
/// it reconnects, so live sessions holding the superseded credential are
/// force-disconnected at once (there is no exempt requester connection on
/// the handle path). Trusted hosts are notified either way.
///
/// # Errors
///
/// Returns `validation_unknown_principal` for an unregistered id, plus the
/// destination, durability, and rollback errors of provisioning.
pub fn rotate_principal_credential(
    runtime: &RelayRuntime,
    principal_id: &str,
    destination: CredentialDestination,
) -> Result<ProvisionedCredential, RelayError> {
    let store_lock = lock_store_transaction(&runtime.state_root);
    let _store_guard = store_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut store = PrincipalStore::load(runtime.state_root.as_path())?;
    store.prune_expired(OffsetDateTime::now_utc());
    let Some(existing) = store.find_by_principal_id(principal_id).cloned()
    else {
        return Err(crate::relay::relay_error(
            "validation_unknown_principal",
            "principal_id is not registered; create it with new peer first",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    let sink = stage_credential_sink(
        &destination,
        existing.principal_type,
        principal_id,
        runtime.state_root.as_path(),
    )?;
    let psk = generate_psk();
    let credential_hash = hash_token_sha256(psk.as_str());
    let pending = write_pending_credential(
        &sink,
        psk.as_str(),
        runtime.state_root.as_path(),
    )?;
    store.remove_by_principal_id(principal_id);
    store.insert(PrincipalRecord {
        principal_id: principal_id.to_string(),
        principal_type: existing.principal_type,
        credential_hash,
        scope: existing.scope.clone(),
        expires_at: existing.expires_at.clone(),
        metadata: existing.metadata.clone(),
    });
    if let Err(error) = store.persist() {
        if !is_durability_uncertain(&error) {
            if let Some(pending) = pending {
                pending.abort();
            }
            return Err(error);
        }
        store.remove_by_principal_id(principal_id);
        store.insert(existing.clone());
        match store.persist() {
            Ok(()) => {
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            },
            Err(rollback) if is_durability_uncertain(&rollback) => {
                if let Some(pending) = pending {
                    pending.abort();
                }
                return Err(error);
            },
            Err(rollback) => {
                match pending {
                    None => {},
                    Some(pending) => {
                        let _ = pending.commit();
                    },
                }
                let (revoked_connections, notified_hosts) =
                    revoke_superseded_credential(
                        &runtime.delivery,
                        principal_id,
                        "",
                    );
                emit_inscription(
                    "relay.identity.psk_rotate_failed",
                    &json!({
                        "principal_id": principal_id,
                        "revoked_connections": revoked_connections,
                        "notified_hosts": notified_hosts,
                    }),
                );
                return Err(credential_rollback_failed(error, rollback));
            },
        }
    }
    let written_path = match pending {
        None => None,
        Some(pending) => match pending.commit() {
            Ok(path) => Some(PathBuf::from(path)),
            Err(error) if is_credential_uncertain(&error) => {
                let (revoked_connections, notified_hosts) =
                    revoke_superseded_credential(
                        &runtime.delivery,
                        principal_id,
                        "",
                    );
                emit_inscription(
                    "relay.identity.psk_rotated",
                    &json!({
                        "principal_id": principal_id,
                        "revoked_connections": revoked_connections,
                        "notified_hosts": notified_hosts,
                    }),
                );
                return Err(error);
            },
            Err(error) => {
                store.remove_by_principal_id(principal_id);
                store.insert(existing);
                if let Err(rollback) = store.persist() {
                    if is_durability_uncertain(&rollback) {
                        // The compensation rename published: the restored
                        // prior record is effective and the replacement
                        // credential file never landed, so the old binding
                        // stands with its live sessions untouched. Report
                        // the original credential failure rather than a
                        // double fault.
                        return Err(error);
                    }
                    // The compensation failed before its own publication:
                    // the rotated hash stands effective with no usable
                    // replacement credential. Fail closed — tear down
                    // sessions holding the superseded credential and notify
                    // watchers — and report the double fault for operator
                    // reconciliation.
                    let (revoked_connections, notified_hosts) =
                        revoke_superseded_credential(
                            &runtime.delivery,
                            principal_id,
                            "",
                        );
                    emit_inscription(
                        "relay.identity.psk_rotate_failed",
                        &json!({
                            "principal_id": principal_id,
                            "revoked_connections": revoked_connections,
                            "notified_hosts": notified_hosts,
                        }),
                    );
                    return Err(credential_rollback_failed(error, rollback));
                }
                return Err(error);
            },
        },
    };
    let (revoked_connections, notified_hosts) =
        revoke_superseded_credential(&runtime.delivery, principal_id, "");
    emit_inscription(
        "relay.identity.psk_rotated",
        &json!({
            "principal_id": principal_id,
            "revoked_connections": revoked_connections,
            "notified_hosts": notified_hosts,
        }),
    );
    let returned_psk = if written_path.is_some() {
        None
    } else {
        Some(psk)
    };
    let config_snippet = build_config_snippet(
        existing.principal_type,
        principal_id,
        runtime.state_root.as_path(),
    );
    Ok(ProvisionedCredential {
        principal_id: principal_id.to_string(),
        psk: returned_psk,
        written_path,
        config_snippet,
    })
}

/// Drops a principal registration, revoking its live sessions.
///
/// The store record is removed and persisted; live sessions holding the
/// deleted credential are force-disconnected and watching trusted hosts are
/// notified, the same teardown rotation performs. Credential files are left
/// on disk, mirroring the admin path: material without a record
/// authenticates nothing.
///
/// # Errors
///
/// Returns `validation_unknown_principal` for an unregistered id, and the
/// store's durability errors carrying the effective outcome.
pub fn drop_principal(
    runtime: &RelayRuntime,
    principal_id: &str,
) -> Result<(), RelayError> {
    let store_lock = lock_store_transaction(&runtime.state_root);
    let _store_guard = store_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut store = PrincipalStore::load(runtime.state_root.as_path())?;
    store.prune_expired(OffsetDateTime::now_utc());
    let Some(_) = store.find_by_principal_id(principal_id).cloned() else {
        return Err(crate::relay::relay_error(
            "validation_unknown_principal",
            "principal_id is not registered",
            Some(json!({ "principal_id": principal_id })),
        ));
    };
    store.remove_by_principal_id(principal_id);
    // Persist before revoking: a pre-rename persist failure leaves the
    // principal authenticating and nothing is torn down. A post-rename
    // durability failure already published the removal, so revocation still
    // runs (the record is effectively gone) and the uncertainty is reported
    // instead of success.
    let durability_uncertain = match store.persist() {
        Ok(()) => None,
        Err(error) if is_durability_uncertain(&error) => Some(error),
        Err(error) => return Err(error),
    };
    let revoked_frame = crate::relay::RelayResponse::Error {
        error: crate::relay::relay_error(
            "runtime_identity_revoked",
            "identity was dropped; the credential no longer authenticates",
            Some(serde_json::json!({ "principal_id": principal_id })),
        ),
    };
    let revoked_connections = revoke_streams_for_identity(
        &runtime.delivery,
        principal_id,
        &revoked_frame,
    );
    let revoked_at = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let revoked_event = RelayStreamEvent {
        event_type: "identity.revoked".to_string(),
        target_session: String::new(),
        created_at: revoked_at.clone(),
        payload: json!({
            "principal_id": principal_id,
            "revoked_at": revoked_at,
        }),
    };
    let notified_hosts = notify_trusted_hosts_of_revocation(
        &runtime.delivery,
        principal_id,
        &revoked_event,
    );
    emit_inscription(
        "relay.identity.principal_dropped",
        &json!({
            "principal_id": principal_id,
            "revoked_connections": revoked_connections,
            "notified_hosts": notified_hosts,
        }),
    );
    if let Some(error) = durability_uncertain {
        return Err(error);
    }
    Ok(())
}

/// Descriptive principal data: identity, type, scope, expiry, and metadata.
///
/// Read-only and descriptive only: it carries no credential material and can
/// never be presented as dispatch authority. There is no API that builds a
/// [`VerifiedPrincipal`] from a snapshot — presenting introspected data,
/// including a bare principal id, where verified context is required is
/// rejected by construction (the types do not meet).
#[derive(Clone, Debug, serde::Serialize)]
pub struct PrincipalSnapshot {
    /// The registered `principal_id`.
    pub principal_id: String,
    /// The principal type as a wire token (`session`, `user`,
    /// `application`, `relay`).
    pub principal_type: String,
    /// Ingress scope, meaningful for `application` and `relay` principals.
    pub scope: Option<String>,
    /// Expiry timestamp, if any.
    pub expires_at: Option<String>,
    /// Descriptive metadata persisted at provisioning time.
    pub metadata: HashMap<String, String>,
}

/// Reads one principal's descriptive record from this runtime's identity
/// store.
///
/// Returns `None` for an unregistered or expired id. Never returns
/// credential material: the store keeps hashes alone.
///
/// # Errors
///
/// Returns the store loader's [`RelayError`] when the store file is
/// corrupt.
pub fn introspect_principal(
    runtime: &RelayRuntime,
    principal_id: &str,
) -> Result<Option<PrincipalSnapshot>, RelayError> {
    let store = PrincipalStore::load(runtime.state_root.as_path())?;
    let Some(record) = store.find_by_principal_id(principal_id) else {
        return Ok(None);
    };
    if record.is_expired(OffsetDateTime::now_utc()) {
        return Ok(None);
    }
    Ok(Some(PrincipalSnapshot {
        principal_id: record.principal_id.clone(),
        principal_type: record.principal_type.as_str().to_string(),
        scope: record.scope.clone(),
        expires_at: record.expires_at.clone(),
        metadata: record.metadata.clone(),
    }))
}
