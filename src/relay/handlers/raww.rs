use std::path::{Path, PathBuf};

use serde_json::json;
use uuid::Uuid;

use crate::configuration::{BundleConfiguration, TargetConfiguration};

use super::super::authorization::{
    AuthorizationContext, choose_authorized_ui_sessions, load_authorization_context,
};
use super::super::connection::BundleCatalog;
use super::super::delivery::{QuiescenceOptions, enqueue_async_delivery};
use super::super::routing::{
    Addressing, Capability, OperationProfile, ResolvedRoute, requester_home_namespace,
    resolve_raww_route,
};
use super::super::stream::lookup_registry_session_type;
use super::super::{
    AsyncDeliveryTask, DeliveryPayloadMode, ListedSessionTransport, RelayError, RelayRequest,
    RelayResponse, SCHEMA_VERSION, bare_session_id, canonical_session_id, relay_error,
    unsupported_operation,
};
use super::routed::{load_home_context, resolve_target_bundle, run_target_operation};
use super::sender::{SenderIdentity, resolve_sender_in_namespace};

/// The raww target's bundle, runtime, and the target bundle's authorization
/// (which gates delivery), resolved and existence-validated by `prepare_raww`
/// before authorization.
struct RawwPrepared {
    bundle: BundleConfiguration,
    runtime_directory: PathBuf,
    target_authorization: AuthorizationContext,
}

/// Entry point for the namespace-centric raww path. The requester is resolved and
/// authorized in its **home** namespace (`home_namespace`: its bound bundle, or
/// `GLOBAL`); the target's bundle is loaded separately for delivery, so a
/// cross-namespace raww authorizes the sender in its home rather than a borrowed
/// target bundle. See `dispatch_raww`.
pub(in crate::relay) fn handle_raww_routed(
    home_namespace: &str,
    home_runtime_directory: Option<&Path>,
    request: RelayRequest,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
) -> Result<RelayResponse, RelayError> {
    let RelayRequest::Raww {
        request_id,
        requester_session,
        target_session,
        text,
        no_enter,
    } = request
    else {
        return Err(relay_error(
            "internal_unexpected_request",
            "non-raww request routed to the raww dispatcher",
            None,
        ));
    };

    if target_session.trim().is_empty() {
        return Err(relay_error(
            "validation_invalid_params",
            "target_session must be non-empty",
            Some(json!({
                "field": "target_session",
            })),
        ));
    }
    if text.len() > 32 * 1024 {
        return Err(relay_error(
            "validation_invalid_params",
            "raww text exceeds maximum size of 32 KiB",
            Some(json!({
                "field": "text",
                "max_bytes": 32 * 1024,
                "bytes": text.len(),
            })),
        ));
    }

    // The requester is identified and authorized in its home namespace (operator
    // policy for `GLOBAL`, or the bundle's policy), never a borrowed target bundle.
    let (home_bundle, authorization) = load_home_context(home_namespace, configuration_root)?;
    let requester_session = bare_session_id(requester_session.as_str(), home_namespace);
    let sender = resolve_sender_in_namespace(
        home_bundle.as_ref(),
        &authorization,
        requester_session.as_str(),
        "requester_session",
    )?;

    // The spine owns resolution and authorization; `prepare_raww` validates
    // existence and loads the target bundle, `execute_raww` delivers the input.
    run_target_operation(
        home_namespace,
        &authorization,
        OperationProfile {
            capability: Capability::Raww,
            addressing: Addressing::SingleTarget,
        },
        || {
            resolve_raww_route(
                requester_home_namespace(sender.session_id.as_str(), home_namespace),
                sender.session_id.as_str(),
                target_session.as_str(),
            )
        },
        |route| {
            prepare_raww(
                route,
                home_namespace,
                home_bundle.as_ref(),
                home_runtime_directory,
                configuration_root,
                bundle_catalog,
            )
        },
        |route, prepared| {
            execute_raww(
                route,
                prepared,
                &sender,
                home_namespace,
                text,
                no_enter,
                request_id,
            )
        },
    )
}

/// Resolves the raww target's bundle, validates that the target is a configured
/// member, applies the `can_be_written` capability gate, and loads the target
/// bundle's authorization (which gates delivery). Runs before authorization, so
/// an unknown or transport-incapable target sorts before
/// `authorization_forbidden`.
fn prepare_raww(
    route: &ResolvedRoute,
    home_namespace: &str,
    home_bundle: Option<&BundleConfiguration>,
    home_runtime_directory: Option<&Path>,
    configuration_root: &Path,
    bundle_catalog: &BundleCatalog,
) -> Result<RawwPrepared, RelayError> {
    let target_route = &route.targets[0];
    let target_namespace = target_route.namespace.as_str();
    let target_session_id = target_route
        .session_id
        .as_deref()
        .expect("raww target carries a session id");
    if target_route.is_relay_wide() {
        // A relay-wide principal is delivered as a stream, not a coder session, so
        // it is never a raww target. Capability comes from its unified registry
        // entry (declared `users.toml` principals are registered offline at
        // startup): a registered principal sorts as `unsupported_operation` (its
        // transport carries `can_be_written = false`), an unregistered one as
        // `unknown_target`.
        let Some(session_type) = lookup_registry_session_type(target_session_id) else {
            return Err(relay_error(
                "validation_unknown_target",
                "target_session is not a registered principal",
                Some(json!({ "target_session": target_session_id })),
            ));
        };
        return Err(unsupported_operation(
            target_session_id,
            session_type,
            "can_be_written",
        ));
    }
    let (bundle, runtime_directory) = resolve_target_bundle(
        home_namespace,
        home_bundle,
        home_runtime_directory,
        target_namespace,
        configuration_root,
        bundle_catalog,
    )?;
    let Some(member) = bundle
        .members
        .iter()
        .find(|member| member.id == target_session_id)
    else {
        return Err(relay_error(
            "validation_unknown_target",
            "target_session is not a canonical configured target identifier",
            Some(json!({
                "target_session": canonical_session_id(target_session_id, target_namespace),
            })),
        ));
    };
    // Capability comes from the target's unified registry entry (a configured
    // session is registered statically at startup); a not-yet-registered member
    // falls back to its configured transport, which derives the same flags.
    let target_principal = canonical_session_id(target_session_id, target_namespace);
    let session_type = lookup_registry_session_type(target_principal.as_str())
        .unwrap_or_else(|| member.target.session_type());
    if !session_type.can_be_written() {
        return Err(unsupported_operation(
            target_principal.as_str(),
            session_type,
            "can_be_written",
        ));
    }
    let target_authorization = load_authorization_context(configuration_root, Some(&bundle))?;
    Ok(RawwPrepared {
        bundle,
        runtime_directory,
        target_authorization,
    })
}

/// Enqueues the raw input for asynchronous delivery to the authorized target
/// and builds the immediate `queued` response. The target is guaranteed present
/// by `prepare_raww`. Choice deciders come from the target bundle's
/// authorization, where delivery is gated (the queue bound is a per-bundle
/// constant captured into the ACP chooser at worker construction, not carried
/// per delivery). The terminal delivery outcome is reported out-of-band via
/// `delivery_outcome` stream events; only enqueue-time failures (e.g. an
/// unavailable ACP worker) surface synchronously.
fn execute_raww(
    route: &ResolvedRoute,
    prepared: RawwPrepared,
    sender: &SenderIdentity,
    home_namespace: &str,
    text: String,
    no_enter: bool,
    request_id: Option<String>,
) -> Result<RelayResponse, RelayError> {
    let RawwPrepared {
        bundle: raww_bundle,
        runtime_directory: raww_runtime_directory,
        target_authorization,
    } = prepared;
    let target_session_id = route.targets[0]
        .session_id
        .as_deref()
        .expect("raww target carries a session id");
    let target_member = raww_bundle
        .members
        .iter()
        .find(|member| member.id == target_session_id)
        .expect("raww target existence validated in prepare_raww");

    let transport = match &target_member.target {
        TargetConfiguration::Tmux(_) => ListedSessionTransport::Tmux,
        TargetConfiguration::Acp(_) => ListedSessionTransport::Acp,
        TargetConfiguration::Pty(_) => ListedSessionTransport::Pty,
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            unreachable!("capability gate in prepare_raww rejects can_be_written = false targets")
        }
    };
    let message_id = Uuid::new_v4().to_string();
    let sender_member = sender.to_bundle_member();
    let choice_decider_sessions =
        choose_authorized_ui_sessions(&target_authorization, &raww_bundle);
    let task = AsyncDeliveryTask {
        bundle: raww_bundle.clone(),
        sender_namespace: home_namespace.to_string(),
        sender: sender_member,
        // Raw input does not carry verified sender attribution, and its targets
        // are never UI streams.
        authenticated_identity: None,
        all_target_sessions: vec![canonical_session_id(
            target_member.id.as_str(),
            raww_bundle.bundle_name.as_str(),
        )],
        target_session: target_member.id.clone(),
        message: text,
        message_id: message_id.clone(),
        // Unbounded quiescence wait: an agent turn can run well past 30 seconds,
        // and async delivery's only hard bound is relay lifetime (shutdown).
        quiescence: QuiescenceOptions::for_async(None),
        runtime_directory: raww_runtime_directory,
        payload_mode: DeliveryPayloadMode::RawInput,
        append_enter: !no_enter,
        choice_decider_sessions,
    };

    // Both transports enqueue onto a per-target async worker; `enqueue_async_delivery`
    // branches on transport internally (ACP requires a pre-existing bounded worker;
    // tmux lazily spawns a generic worker). Only enqueue-time failures surface here.
    match &target_member.target {
        TargetConfiguration::Acp(_)
        | TargetConfiguration::Tmux(_)
        | TargetConfiguration::Pty(_) => {
            enqueue_async_delivery(task)?;
        }
        TargetConfiguration::Ui | TargetConfiguration::Pubsub => {
            unreachable!("capability gate in prepare_raww rejects can_be_written = false targets")
        }
    }

    Ok(RelayResponse::Raww {
        schema_version: SCHEMA_VERSION.to_string(),
        status: "queued".to_string(),
        target_session: canonical_session_id(
            target_member.id.as_str(),
            raww_bundle.bundle_name.as_str(),
        ),
        transport,
        request_id,
        message_id: Some(message_id),
    })
}
