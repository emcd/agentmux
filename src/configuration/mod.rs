//! Bundle configuration loading and sender-association helpers.

mod errors;
mod fields;
mod loaders;
mod paths;
mod raw;
mod roots;
mod targets;
mod types;

pub use errors::ConfigurationError;
pub use loaders::{
    infer_sender_from_working_directory, load_bundle_configuration, load_bundle_group_memberships,
    load_policy_ids, load_tui_configuration, load_tui_configuration_file, load_ui_configuration,
};
pub use paths::{
    bundle_configuration_path, bundle_directory_layers, bundles_configuration_directory,
    coders_configuration_path, configuration_layers, effective_bundle_definitions,
    effective_configuration_path, policies_configuration_path, relay_configuration_path,
    tui_configuration_path, ui_configuration_path,
};
pub use roots::{ConfigurationRoots, ConfigurationRootsError, LAYER_SEPARATOR};
pub use types::{
    AcpChannel, AcpTargetConfiguration, BUNDLE_ENVIRONMENT_VARIABLE, BringUpContext,
    BundleConfiguration, BundleGroupMembership, BundleMember, NameValueEntry,
    PromptReadinessTemplate, PtyTargetConfiguration, RESERVED_GROUP_ALL,
    SESSION_ENVIRONMENT_VARIABLE, SessionType, TargetConfiguration, TermProtocol,
    TmuxTargetConfiguration, TuiConfiguration, TuiSession, UiConfiguration,
};

pub(super) const BUNDLE_SCHEMA_VERSION: u32 = 1;
pub(super) const POLICIES_SCHEMA_VERSION: u32 = 1;
/// Subdirectory of a configuration root holding per-tree overrides. It travels
/// with the root, so overriding a file needs no change to how the root was
/// selected — which is the point, since an MCP client's command line is
/// generated and committed and cannot carry per-tree divergence.
pub(super) const OVERLAY_DIRECTORY: &str = "overlay";
pub(super) const CODERS_FILE: &str = "coders.toml";
pub(super) const BUNDLES_DIRECTORY: &str = "bundles";
pub(super) const BUNDLE_EXTENSION: &str = "toml";
pub(super) const USERS_FILE: &str = "users.toml";
pub(super) const UI_FILE: &str = "ui.toml";
pub(super) const POLICIES_FILE: &str = "policies.toml";
pub(super) const RELAY_FILE: &str = "relay.toml";
pub(super) const SESSION_ID_LENGTH_MAX: usize = 31;
pub(super) const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";
