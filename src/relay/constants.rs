use crate::envelope::ENVELOPE_SCHEMA_VERSION;

pub(super) const SCHEMA_VERSION: &str = ENVELOPE_SCHEMA_VERSION;
pub(super) const POLICIES_FILE: &str = "policies.toml";
pub(super) const POLICIES_FORMAT_VERSION: u32 = 1;
pub(super) const GLOBAL_SESSION_SUFFIX: &str = "@GLOBAL";
/// The relay-wide namespace. A sender or target in this namespace has no bundle;
/// it is the home namespace of every relay-wide (`@GLOBAL`) principal.
pub(super) const GLOBAL_NAMESPACE: &str = "GLOBAL";
