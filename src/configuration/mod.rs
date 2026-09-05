//! Bundle configuration loading and sender-association helpers.

mod bindings;
mod errors;
mod fields;
mod loaders;
mod paths;
mod raw;
mod roots;
mod targets;
mod types;

pub use bindings::{ShippedPreset, embedded_binding_preset, shipped_binding_presets};
pub use errors::ConfigurationError;
pub use loaders::{
    infer_sender_from_working_directory, inject_spawn_state_directory, load_bundle_configuration,
    load_bundle_group_memberships, load_policy_ids, load_tui_configuration,
    load_tui_configuration_file, load_ui_configuration,
};
pub use paths::{
    bundle_configuration_path, bundle_directory_layers, coders_configuration_path,
    effective_bundle_definitions, effective_configuration_path, policies_configuration_path,
    relay_configuration_path, supplied_configuration_path, supplied_root_configuration_sources,
    tui_configuration_path, ui_configuration_path,
};
pub use roots::{ConfigurationRoots, ConfigurationRootsError, LAYER_SEPARATOR};
pub use types::{
    AcpChannel, AcpTargetConfiguration, BUNDLE_ENVIRONMENT_VARIABLE, BringUpContext,
    BundleConfiguration, BundleGroupMembership, BundleMember,
    CONFIGURATION_DIRECTORY_ENVIRONMENT_VARIABLE, ContextValue, INHERITED_CONTEXT_VARIABLE_NAMES,
    LayerRepresentationFault, NameValueEntry, PromptReadinessTemplate, PtyTargetConfiguration,
    RESERVED_GROUP_ALL, SESSION_ENVIRONMENT_VARIABLE, STATE_DIRECTORY_ENVIRONMENT_VARIABLE,
    SessionType, TargetConfiguration, TermProtocol, TmuxTargetConfiguration, TuiConfiguration,
    TuiSession, UiConfiguration, UnrepresentableLayer,
};

pub(super) const BUNDLE_SCHEMA_VERSION: u32 = 1;
pub(super) const POLICIES_SCHEMA_VERSION: u32 = 1;
pub(super) const CODERS_FILE: &str = "coders.toml";
pub(super) const BUNDLES_DIRECTORY: &str = "bundles";
pub(super) const BUNDLE_EXTENSION: &str = "toml";
pub(super) const USERS_FILE: &str = "users.toml";
pub(super) const UI_FILE: &str = "ui.toml";
/// Association overrides (`mcp.toml`), naming the bundle and session an MCP
/// server binds to.
///
/// Declared here with the other artifact names rather than beside its loader:
/// the source inventory is built from this list, and a name held somewhere else
/// is a name the inventory silently omits.
pub(super) const ASSOCIATION_FILE: &str = "mcp.toml";
pub(super) const POLICIES_FILE: &str = "policies.toml";
pub(super) const RELAY_FILE: &str = "relay.toml";
pub(super) const SESSION_ID_LENGTH_MAX: usize = 31;
pub(super) const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";
