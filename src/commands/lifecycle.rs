use std::env;

use serde_json::{Map, Value, json};

use crate::{
    configuration::{load_bundle_configuration, load_bundle_group_memberships},
    relay::{RelayRequest, RelayResponse, request_relay},
    runtime::{
        association::{WorkspaceContext, load_local_mcp_overrides},
        error::RuntimeError,
        paths::BundleRuntimePaths,
        starter::ensure_starter_configuration_layout,
    },
};

use super::{
    LifecycleAction, LifecycleArguments, LifecycleSelector, LifecycleTransitionBundle,
    LifecycleTransitionSummary, RuntimeArguments, shared,
};

pub(super) fn run_bundle_lifecycle(
    action: LifecycleAction,
    arguments: &[String],
) -> Result<(), RuntimeError> {
    let parsed = parse_lifecycle_arguments(action, arguments)?;
    let current_directory = env::current_dir()
        .map_err(|source| RuntimeError::io("resolve current working directory", source))?;
    let workspace = WorkspaceContext::discover(&current_directory)?;
    let local_overrides = load_local_mcp_overrides(&workspace.workspace_root)?;
    let roots = shared::resolve_roots(&parsed.runtime, &workspace, local_overrides.as_ref())?;
    ensure_starter_configuration_layout(&roots.configuration_root)?;

    let selected_bundles = resolve_selected_bundles(&roots.configuration_root, &parsed.selector)?;
    let mut bundles = Vec::<LifecycleTransitionBundle>::with_capacity(selected_bundles.len());
    for bundle_name in selected_bundles {
        let paths = BundleRuntimePaths::resolve(&roots.state_root, bundle_name.as_str())?;
        let relay_request = match parsed.action {
            LifecycleAction::Up => RelayRequest::Up,
            LifecycleAction::Down => RelayRequest::Down,
        };
        let response = request_relay(&paths.relay_socket, &relay_request)
            .map_err(|source| shared::map_relay_request_failure(&paths.relay_socket, source))?;
        match response {
            RelayResponse::Lifecycle {
                bundles: relay_bundles,
                ..
            } => {
                let Some(entry) = relay_bundles.first() else {
                    return Err(RuntimeError::validation(
                        "internal_unexpected_failure",
                        "relay returned lifecycle payload with no bundle entries".to_string(),
                    ));
                };
                bundles.push(LifecycleTransitionBundle {
                    bundle_name: entry.bundle_name.clone(),
                    outcome: entry.outcome.clone(),
                    reason_code: entry.reason_code.clone(),
                    reason: entry.reason.clone(),
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
        "Usage: agentmux up (<bundle-id> | --group GROUP) [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

pub(super) fn print_down_help() {
    println!(
        "Usage: agentmux down (<bundle-id> | --group GROUP) [--config-directory PATH] [--state-directory PATH] [--inscriptions-directory PATH|--logs-directory PATH] [--repository-root PATH]"
    );
}

fn parse_lifecycle_arguments(
    action: LifecycleAction,
    arguments: &[String],
) -> Result<LifecycleArguments, RuntimeError> {
    let mut parsed = LifecycleArguments {
        action,
        selector: LifecycleSelector::Bundle(String::new()),
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
        (Some(bundle_name), None) => LifecycleSelector::Bundle(bundle_name),
        (None, Some(group_name)) => {
            shared::validate_group_selector_name(group_name.as_str())?;
            LifecycleSelector::Group(group_name)
        }
    };
    Ok(parsed)
}

fn resolve_selected_bundles(
    configuration_root: &std::path::Path,
    selector: &LifecycleSelector,
) -> Result<Vec<String>, RuntimeError> {
    match selector {
        LifecycleSelector::Bundle(bundle_name) => {
            let _bundle = load_bundle_configuration(configuration_root, bundle_name)
                .map_err(shared::map_bundle_load_error)?;
            Ok(vec![bundle_name.to_string()])
        }
        LifecycleSelector::Group(group_name) => {
            let memberships = load_bundle_group_memberships(configuration_root)
                .map_err(shared::map_bundle_load_error)?;
            shared::resolve_group_bundles(memberships, group_name)
        }
    }
}

fn build_transition_summary(
    action: LifecycleAction,
    bundles: Vec<LifecycleTransitionBundle>,
) -> LifecycleTransitionSummary {
    let changed_bundle_count = bundles
        .iter()
        .filter(|bundle| matches!(bundle.outcome.as_str(), "hosted" | "unhosted"))
        .count();
    let skipped_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "skipped")
        .count();
    let failed_bundle_count = bundles
        .iter()
        .filter(|bundle| bundle.outcome == "failed")
        .count();
    LifecycleTransitionSummary {
        schema_version: 1,
        action: match action {
            LifecycleAction::Up => "up".to_string(),
            LifecycleAction::Down => "down".to_string(),
        },
        bundles,
        changed_bundle_count,
        skipped_bundle_count,
        failed_bundle_count,
        changed_any: changed_bundle_count > 0,
    }
}

fn transition_summary_payload(summary: &LifecycleTransitionSummary) -> Value {
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

fn render_transition_summary(summary: &LifecycleTransitionSummary) {
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
        "agentmux {} summary changed={} skipped={} failed={} changed_any={}",
        summary.action,
        summary.changed_bundle_count,
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
    }
}
