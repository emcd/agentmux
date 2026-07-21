use std::{
    collections::{HashMap, HashSet},
    fs,
    path::Path,
};

use super::{
    BUNDLE_SCHEMA_VERSION, BUNDLES_DIRECTORY, ConfigurationError, POLICIES_SCHEMA_VERSION,
    fields::{
        canonicalize_best_effort, normalize_field, normalize_global_session_id,
        validate_bundle_groups, validate_format_version, validate_session_id,
    },
    paths::{
        bundle_configuration_path, coders_configuration_path, policies_configuration_path,
        tui_configuration_path, ui_configuration_path,
    },
    raw::{
        Coder, CoderTarget, RawBundleFile, RawCoder, RawCodersFile, RawPoliciesFile, RawUiFile,
        RawUsersFile, RawUsersSession,
    },
    targets::{
        build_session_target, select_marker_session_type, validate_acp_target,
        validate_environment_entries, validate_pty_target, validate_tmux_target,
    },
    types::{
        BundleConfiguration, BundleGroupMembership, BundleMember, NameValueEntry, TuiConfiguration,
        TuiSession, UiConfiguration,
    },
};

/// Loads bundle-group membership metadata for configured bundles.
///
/// # Errors
///
/// Returns `ConfigurationError` for malformed bundle files and I/O failures.
pub fn load_bundle_group_memberships(
    configuration_root: &Path,
) -> Result<Vec<BundleGroupMembership>, ConfigurationError> {
    let bundles_directory = configuration_root.join(BUNDLES_DIRECTORY);
    if !bundles_directory.exists() {
        return Ok(Vec::new());
    }
    let mut bundle_names = fs::read_dir(&bundles_directory)
        .map_err(|source| {
            ConfigurationError::io(
                format!("read bundle directory {}", bundles_directory.display()),
                source,
            )
        })?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.path().file_name().map(ToOwned::to_owned))
        .filter_map(|name| name.to_str().map(ToOwned::to_owned))
        .filter(|name| name.ends_with(".toml"))
        .filter_map(|name| name.strip_suffix(".toml").map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    bundle_names.sort_unstable();

    let mut memberships = Vec::with_capacity(bundle_names.len());
    for bundle_name in bundle_names {
        let bundle_path = bundle_configuration_path(configuration_root, &bundle_name);
        let bundle_raw = fs::read_to_string(&bundle_path).map_err(|source| {
            ConfigurationError::io(format!("read {}", bundle_path.display()), source)
        })?;
        let bundle_file = toml::from_str::<RawBundleFile>(&bundle_raw).map_err(|source| {
            ConfigurationError::InvalidConfiguration {
                path: bundle_path.clone(),
                message: source.to_string(),
            }
        })?;
        validate_format_version(
            bundle_file.format_version,
            BUNDLE_SCHEMA_VERSION,
            &bundle_path,
        )?;
        if bundle_file.sessions.is_empty() {
            continue;
        }
        let groups = validate_bundle_groups(&bundle_file.groups, &bundle_path)?;
        memberships.push(BundleGroupMembership {
            bundle_name,
            autostart: bundle_file.autostart,
            groups,
        });
    }
    Ok(memberships)
}

/// Loads one bundle configuration and applies schema validation.
///
/// # Errors
///
/// Returns `ConfigurationError` for unknown bundles, invalid schema, and I/O.
pub fn load_bundle_configuration(
    configuration_root: &Path,
    bundle_name: &str,
) -> Result<BundleConfiguration, ConfigurationError> {
    let coders_path = coders_configuration_path(configuration_root);
    let bundle_path = bundle_configuration_path(configuration_root, bundle_name);

    if !bundle_path.exists() {
        return Err(ConfigurationError::UnknownBundle {
            bundle_name: bundle_name.to_string(),
            path: bundle_path,
        });
    }

    let coders_raw = fs::read_to_string(&coders_path).map_err(|source| {
        ConfigurationError::io(format!("read {}", coders_path.display()), source)
    })?;
    let bundle_raw = fs::read_to_string(&bundle_path).map_err(|source| {
        ConfigurationError::io(format!("read {}", bundle_path.display()), source)
    })?;

    let coders_file = toml::from_str::<RawCodersFile>(&coders_raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: coders_path.clone(),
            message: source.to_string(),
        }
    })?;
    let bundle_file = toml::from_str::<RawBundleFile>(&bundle_raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: bundle_path.clone(),
            message: source.to_string(),
        }
    })?;

    validate_loaded_configuration(
        bundle_name,
        coders_file,
        &coders_path,
        bundle_file,
        &bundle_path,
    )
}

/// Loads global user configuration from `<config-root>/users.toml`.
///
/// # Errors
///
/// Returns `ConfigurationError` when the file exists but is malformed.
pub fn load_tui_configuration(
    configuration_root: &Path,
) -> Result<Option<TuiConfiguration>, ConfigurationError> {
    load_tui_configuration_file(&tui_configuration_path(configuration_root))
}

/// Loads global user configuration from an explicit file path.
///
/// # Errors
///
/// Returns `ConfigurationError` when the file exists but is malformed.
pub fn load_tui_configuration_file(
    path: &Path,
) -> Result<Option<TuiConfiguration>, ConfigurationError> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(path)
        .map_err(|source| ConfigurationError::io(format!("read {}", path.display()), source))?;
    let parsed = toml::from_str::<RawUsersFile>(&raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })?;

    let default_session = parsed
        .default_session
        .as_deref()
        .map(normalize_field)
        .filter(|value| !value.is_empty())
        .map(|value| normalize_global_session_id(path, value))
        .transpose()?;
    let sessions = validate_tui_sessions(parsed.sessions, path)?;

    Ok(Some(TuiConfiguration {
        default_session,
        sessions,
    }))
}

/// Loads UI-surface configuration from `<config-root>/ui.toml`.
///
/// UI-surface defaults (currently `default-bundle`) are read-only operator
/// preferences, kept separate from the `users.toml` identity/policy file. A
/// missing `ui.toml` resolves to `Ok(None)` (no configured defaults); a
/// malformed file fails fast with a structured error naming the path.
///
/// # Errors
///
/// Returns `ConfigurationError` when the file exists but is malformed.
pub fn load_ui_configuration(
    configuration_root: &Path,
) -> Result<Option<UiConfiguration>, ConfigurationError> {
    let path = ui_configuration_path(configuration_root);
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path)
        .map_err(|source| ConfigurationError::io(format!("read {}", path.display()), source))?;
    let parsed = toml::from_str::<RawUiFile>(&raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: path.clone(),
            message: source.to_string(),
        }
    })?;

    let default_bundle = parsed
        .default_bundle
        .as_deref()
        .map(normalize_field)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    Ok(Some(UiConfiguration { default_bundle }))
}

/// Loads known policy preset identifiers from `<config-root>/policies.toml`.
///
/// # Errors
///
/// Returns `ConfigurationError` when the artifact is missing or malformed.
pub fn load_policy_ids(configuration_root: &Path) -> Result<HashSet<String>, ConfigurationError> {
    let path = policies_configuration_path(configuration_root);
    let raw = fs::read_to_string(&path)
        .map_err(|source| ConfigurationError::io(format!("read {}", path.display()), source))?;
    let parsed = toml::from_str::<RawPoliciesFile>(&raw).map_err(|source| {
        ConfigurationError::InvalidConfiguration {
            path: path.clone(),
            message: source.to_string(),
        }
    })?;
    validate_format_version(parsed.format_version, POLICIES_SCHEMA_VERSION, &path)?;

    let mut unique = HashSet::<String>::new();
    for policy in parsed.policies {
        let policy_id = normalize_field(policy.id.as_str());
        if policy_id.is_empty() {
            return Err(ConfigurationError::invalid(
                &path,
                "policy id must be non-empty",
            ));
        }
        if !unique.insert(policy_id.to_string()) {
            return Err(ConfigurationError::invalid(
                &path,
                format!("duplicate policy id '{policy_id}'"),
            ));
        }
    }
    Ok(unique)
}

/// Infers sender session from bundle member working-directory matches.
///
/// # Errors
///
/// Returns `ConfigurationError::AmbiguousSender` when more than one member
/// matches the same directory.
pub fn infer_sender_from_working_directory(
    bundle: &BundleConfiguration,
    working_directory: &Path,
) -> Result<Option<String>, ConfigurationError> {
    let target = canonicalize_best_effort(working_directory);
    let mut matches = Vec::new();

    for member in &bundle.members {
        let Some(member_directory) = member.working_directory.as_ref() else {
            continue;
        };
        if canonicalize_best_effort(member_directory) == target {
            matches.push(member.id.clone());
        }
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Err(ConfigurationError::AmbiguousSender {
            working_directory: target,
            matches,
        }),
    }
}

fn validate_loaded_configuration(
    expected_bundle_name: &str,
    coders_file: RawCodersFile,
    coders_path: &Path,
    bundle_file: RawBundleFile,
    bundle_path: &Path,
) -> Result<BundleConfiguration, ConfigurationError> {
    validate_format_version(
        coders_file.format_version,
        BUNDLE_SCHEMA_VERSION,
        coders_path,
    )?;
    validate_format_version(
        bundle_file.format_version,
        BUNDLE_SCHEMA_VERSION,
        bundle_path,
    )?;

    let coders = validate_coders(coders_file.coders, coders_path)?;
    let groups = validate_bundle_groups(&bundle_file.groups, bundle_path)?;
    validate_environment_entries(
        &bundle_file.environment,
        bundle_path,
        &format!("bundle '{expected_bundle_name}'"),
    )?;

    if bundle_file.sessions.is_empty() {
        return Err(ConfigurationError::invalid(
            bundle_path,
            "sessions must contain at least one session",
        ));
    }

    let mut session_ids = HashSet::new();
    let mut session_names = HashSet::new();
    let mut members = Vec::with_capacity(bundle_file.sessions.len());

    for session in &bundle_file.sessions {
        let session_id = normalize_field(session.id.as_str());
        if session_id.is_empty() {
            return Err(ConfigurationError::invalid(
                bundle_path,
                "session id must be non-empty",
            ));
        }
        validate_session_id(bundle_path, session_id)?;
        if !session_ids.insert(session_id.to_string()) {
            return Err(ConfigurationError::invalid(
                bundle_path,
                format!("duplicate session id '{session_id}'"),
            ));
        }

        let session_name = session
            .name
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty());
        if let Some(session_name) = session_name
            && !session_names.insert(session_name.to_string())
        {
            return Err(ConfigurationError::invalid(
                bundle_path,
                format!("duplicate session name '{session_name}'"),
            ));
        }

        if session.directory.as_os_str().is_empty() {
            return Err(ConfigurationError::invalid(
                bundle_path,
                format!("session '{session_id}' directory must be non-empty"),
            ));
        }

        let policy_id = session
            .policy
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        let (target, coder_session_id) =
            build_session_target(session, &coders, coders_path, bundle_path, session_id)?;

        validate_environment_entries(
            &session.environment,
            bundle_path,
            &format!("session '{session_id}'"),
        )?;

        // Merge the three environment layers once at load. `build_session_target`
        // has already validated that a coder-backed session's coder resolves, so
        // this lookup cannot miss for a coder session; coder-less members simply
        // carry an empty coder layer.
        let coder_environment = session
            .coder
            .as_deref()
            .map(normalize_field)
            .filter(|coder_id| !coder_id.is_empty())
            .and_then(|coder_id| coders.get(coder_id))
            .map(|coder| coder.environment.as_slice())
            .unwrap_or_default();
        let environment = merge_environment(
            coder_environment,
            &bundle_file.environment,
            &session.environment,
        );

        members.push(BundleMember {
            id: session_id.to_string(),
            name: session_name.map(ToString::to_string),
            working_directory: Some(session.directory.clone()),
            target,
            coder_session_id,
            policy_id,
            environment,
        });
    }

    Ok(BundleConfiguration {
        schema_version: BUNDLE_SCHEMA_VERSION.to_string(),
        bundle_name: expected_bundle_name.to_string(),
        autostart: bundle_file.autostart,
        groups,
        members,
    })
}

fn validate_tui_sessions(
    sessions: Vec<RawUsersSession>,
    path: &Path,
) -> Result<Vec<TuiSession>, ConfigurationError> {
    let mut unique = HashSet::<String>::new();
    let mut validated = Vec::<TuiSession>::with_capacity(sessions.len());
    for session in sessions {
        let selector_id = normalize_field(session.id.as_str());
        if selector_id.is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                "users session id must be non-empty",
            ));
        }
        let canonical_id = normalize_global_session_id(path, selector_id)?;
        if !unique.insert(canonical_id.clone()) {
            return Err(ConfigurationError::invalid(
                path,
                format!("duplicate users session id '{canonical_id}'"),
            ));
        }

        let policy_id = normalize_field(session.policy.as_str());
        if policy_id.is_empty() {
            return Err(ConfigurationError::invalid(
                path,
                format!("users session '{canonical_id}' policy must be non-empty"),
            ));
        }
        let session_type = select_marker_session_type(
            session.ui.is_some(),
            session.pubsub.is_some(),
            path,
            canonical_id.as_str(),
        )?;
        let name = session
            .name
            .as_deref()
            .map(normalize_field)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        validated.push(TuiSession {
            id: canonical_id,
            name,
            policy: policy_id.to_string(),
            session_type,
        });
    }
    Ok(validated)
}

fn validate_coders(
    coders: Vec<RawCoder>,
    coders_path: &Path,
) -> Result<HashMap<String, Coder>, ConfigurationError> {
    if coders.is_empty() {
        return Err(ConfigurationError::invalid(
            coders_path,
            "coders must contain at least one coder",
        ));
    }

    let mut unique = HashMap::new();
    for coder in coders {
        let coder_id = normalize_field(coder.id.as_str());
        if coder_id.is_empty() {
            return Err(ConfigurationError::invalid(
                coders_path,
                "coder id must be non-empty",
            ));
        }
        if unique.contains_key(coder_id) {
            return Err(ConfigurationError::invalid(
                coders_path,
                format!("duplicate coder id '{coder_id}'"),
            ));
        }

        let target = match (coder.tmux, coder.acp, coder.pty) {
            (Some(tmux), None, None) => {
                CoderTarget::Tmux(validate_tmux_target(tmux, coders_path, coder_id)?)
            }
            (None, Some(acp), None) => {
                CoderTarget::Acp(validate_acp_target(acp, coders_path, coder_id)?)
            }
            (None, None, Some(pty)) => {
                CoderTarget::Pty(validate_pty_target(pty, coders_path, coder_id)?)
            }
            (None, None, None) => {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!(
                        "coder '{coder_id}' must define exactly one target table ([coders.tmux], [coders.acp], or [coders.pty])"
                    ),
                ));
            }
            _ => {
                return Err(ConfigurationError::invalid(
                    coders_path,
                    format!(
                        "coder '{coder_id}' defines multiple target tables; expected exactly one"
                    ),
                ));
            }
        };

        validate_environment_entries(
            &coder.environment,
            coders_path,
            &format!("coder '{coder_id}'"),
        )?;

        unique.insert(
            coder_id.to_string(),
            Coder {
                target,
                environment: coder.environment,
            },
        );
    }

    Ok(unique)
}

/// Merges the coder, bundle, and session environment layers into the effective
/// environment for one member. Precedence is per-variable, most-specific-wins
/// (session > bundle > coder); non-colliding names union across the layers.
/// Declaration order is preserved: a name keeps the position of its first
/// appearance (coder, then new bundle names, then new session names), while a
/// more-specific layer overrides the value in place.
fn merge_environment(
    coder: &[NameValueEntry],
    bundle: &[NameValueEntry],
    session: &[NameValueEntry],
) -> Vec<NameValueEntry> {
    let mut merged: Vec<NameValueEntry> = Vec::new();
    for layer in [coder, bundle, session] {
        for entry in layer {
            if let Some(existing) = merged.iter_mut().find(|current| current.name == entry.name) {
                existing.value = entry.value.clone();
            } else {
                merged.push(entry.clone());
            }
        }
    }
    merged
}
