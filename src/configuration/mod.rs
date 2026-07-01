//! Bundle configuration loading and sender-association helpers.

mod errors;
mod fields;
mod loaders;
mod paths;
mod raw;
mod targets;
mod types;

pub use errors::ConfigurationError;
pub use loaders::{
    infer_sender_from_working_directory, load_bundle_configuration, load_bundle_group_memberships,
    load_policy_ids, load_tui_configuration, load_tui_configuration_file,
};
pub use paths::{
    bundle_configuration_path, bundles_configuration_directory, coders_configuration_path,
    policies_configuration_path, tui_configuration_path,
};
pub use types::{
    AcpChannel, AcpTargetConfiguration, BundleConfiguration, BundleGroupMembership, BundleMember,
    NameValueEntry, PromptReadinessTemplate, PtyTargetConfiguration, RESERVED_GROUP_ALL,
    SessionType, TargetConfiguration, TmuxTargetConfiguration, TuiConfiguration, TuiSession,
};

pub(super) const BUNDLE_SCHEMA_VERSION: u32 = 1;
pub(super) const POLICIES_SCHEMA_VERSION: u32 = 1;
pub(super) const CODERS_FILE: &str = "coders.toml";
pub(super) const BUNDLES_DIRECTORY: &str = "bundles";
pub(super) const BUNDLE_EXTENSION: &str = "toml";
pub(super) const USERS_FILE: &str = "users.toml";
pub(super) const POLICIES_FILE: &str = "policies.toml";
pub(super) const SESSION_ID_LENGTH_MAX: usize = 31;
pub(super) const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";
