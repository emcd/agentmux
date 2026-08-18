use serde_json::{Map, Value, json};

use crate::{
    configuration::{ConfigurationRoots, load_bundle_configuration, load_bundle_group_memberships},
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        error::RuntimeError, paths::RelayRuntimePaths,
        starter::ensure_starter_configuration_layout, tui_session::resolve_tui_session_identity,
    },
};

use super::{
    BundleAction, BundleArguments, BundleSelector, BundleTransitionResult, BundleTransitionSummary,
    RuntimeArguments, shared,
};

pub(super) fn run_bundle_command(
    action: BundleAction,
    arguments: &[String],
) -> Result<(), RuntimeError> {
    let parsed = parse_bundle_arguments(action, arguments)?;
    let roots = shared::resolve_roots(&parsed.runtime)?;
    ensure_starter_configuration_layout(&roots)?;

    let selected_bundles = resolve_selected_bundles(&roots.configuration_roots, &parsed.selector)?;
    let relay_paths = RelayRuntimePaths::resolve(&roots.state_root);
    let mut bundles = Vec::<BundleTransitionResult>::with_capacity(selected_bundles.len());
    for bundle_name in selected_bundles {
        let resolved_operator = resolve_tui_session_identity(
            &roots.configuration_roots,
            Some(bundle_name.as_str()),
            None,
        )?;
        let relay_request = match parsed.action {
            BundleAction::Up => RelayRequest::Up,
            BundleAction::Down => RelayRequest::Down,
        };
        let response = request_relay(
            &relay_paths.relay_socket,
            bundle_name.as_str(),
            resolved_operator.session_id.as_str(),
            &relay_request,
        )
        .map_err(|source| shared::map_relay_request_failure(&relay_paths.relay_socket, source))?;
        match response {
            RelayResponse::BundleTransition {
                bundles: relay_bundles,
                ..
            } => {
                let Some(entry) = relay_bundles.first() else {
                    return Err(RuntimeError::validation(
                        "internal_unexpected_failure",
                        "relay returned bundle transition payload with no bundle entries"
                            .to_string(),
                    ));
                };
                bundles.push(BundleTransitionResult {
                    bundle_name: entry.bundle_name.clone(),
                    outcome: entry.outcome.clone(),
                    reason_code: entry.reason_code.clone(),
                    reason: entry.reason.clone(),
                    details: entry.details.clone(),
                });
            }
            RelayResponse::Error { error } => return Err(shared::map_relay_error(error)),
            other => {
                return Err(RuntimeError::validation(
                    "internal_unexpected_failure",
                    format!("relay returned unexpected response variant: {other:?}"),
                ));
            }
        }
    }

    let summary = build_transition_summary(parsed.action, bundles);
    render_transition_summary(&summary);
    Ok(())
}

pub(super) fn print_up_help() {
    println!(
        "Usage: agentmux up (<bundle-id> | --group GROUP) [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]"
    );
}

pub(super) fn print_down_help() {
    println!(
        "Usage: agentmux down (<bundle-id> | --group GROUP) [--configuration-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH]"
    );
}

fn parse_bundle_arguments(
    action: BundleAction,
    arguments: &[String],
) -> Result<BundleArguments, RuntimeError> {
    let mut parsed = BundleArguments {
        action,
        selector: BundleSelector::Bundle(String::new()),
        runtime: RuntimeArguments::default(),
    };
    let mut positional_bundle = None::<String>;
    let mut group_name = None::<String>;
    let mut index = 0usize;
    while index < arguments.len() {
        if shared::parse_runtime_flag(arguments, &mut index, &mut parsed.runtime)? {
            index += 1;
            continue;
        }
        match arguments[index].as_str() {
            "--group" => group_name = Some(shared::take_value(arguments, &mut index, "--group")?),
            value if !value.starts_with('-') => {
                if positional_bundle.is_some() {
                    return Err(RuntimeError::InvalidArgument {
                        argument: value.to_string(),
                        message: "unknown argument".to_string(),
                    });
                }
                positional_bundle = Some(value.to_string());
            }
            unknown => {
                return Err(RuntimeError::InvalidArgument {
                    argument: unknown.to_string(),
                    message: "unknown argument".to_string(),
                });
            }
        }
        index += 1;
    }
    parsed.selector = match (positional_bundle, group_name) {
        (Some(_), Some(_)) => {
            return Err(RuntimeError::validation(
                "validation_conflicting_selectors",
                "provide either positional <bundle-id> or --group <GROUP>, not both".to_string(),
            ));
        }
        (None, None) => {
            return Err(RuntimeError::InvalidArgument {
                argument: "<bundle-id>|--group".to_string(),
                message: "missing selector".to_string(),
            });
        }
        (Some(bundle_name), None) => BundleSelector::Bundle(bundle_name),
        (None, Some(group_name)) => {
            shared::validate_group_selector_name(group_name.as_str())?;
            BundleSelector::Group(group_name)
        }
    };
    Ok(parsed)
}

fn resolve_selected_bundles(
    configuration_roots: &ConfigurationRoots,
    selector: &BundleSelector,
) -> Result<Vec<String>, RuntimeError> {
    match selector {
        BundleSelector::Bundle(bundle_name) => {
            let _bundle = load_bundle_configuration(configuration_roots, bundle_name)
                .map_err(shared::map_bundle_load_error)?;
            Ok(vec![bundle_name.to_string()])
        }
        BundleSelector::Group(group_name) => {
            let memberships = load_bundle_group_memberships(configuration_roots)
                .map_err(shared::map_bundle_load_error)?;
            shared::resolve_group_bundles(memberships, group_name)
        }
    }
}

fn build_transition_summary(
    action: BundleAction,
    bundles: Vec<BundleTransitionResult>,
) -> BundleTransitionSummary {
    let changed_bundle_count = bundles
        .iter()
        .filter(|bundle| matches!(bundle.outcome.as_str(), "hosted" | "unhosted"))
        .count();
    let degraded_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "degraded")
        .count();
    let skipped_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "skipped")
        .count();
    let failed_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "failed")
        .count();
    BundleTransitionSummary {
        schema_version: 1,
        action: match action {
            BundleAction::Up => "up".to_string(),
            BundleAction::Down => "down".to_string(),
        },
        bundles,
        changed_bundle_count,
        degraded_bundle_count,
        skipped_bundle_count,
        failed_bundle_count,
        // A degraded bundle came up; it just did not come up whole. Leaving it
        // out would report `changed_any=false` for a transition that started
        // sessions.
        changed_any: changed_bundle_count + degraded_bundle_count > 0,
    }
}

fn transition_summary_payload(summary: &BundleTransitionSummary) -> Value {
    let mut payload = Map::<String, Value>::new();
    payload.insert("schema_version".to_string(), json!(summary.schema_version));
    payload.insert("action".to_string(), json!(summary.action));
    payload.insert(
        "bundles".to_string(),
        Value::Array(
            summary
                .bundles
                .iter()
                .map(|bundle| {
                    json!({
                        "bundle_name": bundle.bundle_name,
                        "outcome": bundle.outcome,
                        "reason_code": bundle.reason_code,
                        "reason": bundle.reason,
                        "details": bundle.details,
                    })
                })
                .collect::<Vec<_>>(),
        ),
    );
    payload.insert(
        "changed_bundle_count".to_string(),
        json!(summary.changed_bundle_count),
    );
    payload.insert(
        "degraded_bundle_count".to_string(),
        json!(summary.degraded_bundle_count),
    );
    payload.insert(
        "skipped_bundle_count".to_string(),
        json!(summary.skipped_bundle_count),
    );
    payload.insert(
        "failed_bundle_count".to_string(),
        json!(summary.failed_bundle_count),
    );
    payload.insert("changed_any".to_string(), json!(summary.changed_any));
    Value::Object(payload)
}

fn render_transition_summary(summary: &BundleTransitionSummary) {
    match serde_json::to_string(&transition_summary_payload(summary)) {
        Ok(encoded) => println!("{encoded}"),
        Err(source) => {
            eprintln!(
                "agentmux {}: failed to encode summary json: {source}",
                summary.action
            );
        }
    }
    println!(
        "agentmux {} summary changed={} degraded={} skipped={} failed={} changed_any={}",
        summary.action,
        summary.changed_bundle_count,
        summary.degraded_bundle_count,
        summary.skipped_bundle_count,
        summary.failed_bundle_count,
        summary.changed_any,
    );
    for bundle in &summary.bundles {
        match (bundle.reason_code.as_deref(), bundle.reason.as_deref()) {
            (Some(reason_code), Some(reason)) => {
                println!(
                    "bundle={} outcome={} reason_code={} reason={}",
                    bundle.bundle_name, bundle.outcome, reason_code, reason
                );
            }
            (Some(reason_code), None) => {
                println!(
                    "bundle={} outcome={} reason_code={}",
                    bundle.bundle_name, bundle.outcome, reason_code
                );
            }
            _ => println!("bundle={} outcome={}", bundle.bundle_name, bundle.outcome),
        }
        shared::render_failed_sessions(bundle.bundle_name.as_str(), bundle.details.as_ref());
    }
}
