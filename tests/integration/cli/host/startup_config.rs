//! Bundle-configuration writers shared by the startup clusters ([`super::startup`]
//! and [`super::startup_failures`]). Each writer lays out a config root whose
//! autostart bundle exercises one startup shape: mixed tmux/ACP failure,
//! total ACP failure, invalid policy scope, or a tmux member that dies
//! between session creation and the ownership mark.

use std::{fs, path::Path};

pub(super) fn write_bundle_configuration_with_tmux_and_acp_failure(
    config_root: &Path,
    bundle_name: &str,
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "tmux-default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"

[[coders]]
id = "acp-broken"

[coders.acp]
channel = "stdio"
command = "/definitely/missing/agentmux-acp"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
# The CLI lists this bundle as the relay-wide `user@GLOBAL` operator, whose home
# namespace is GLOBAL; reaching into a bundle is cross-namespace and requires
# all.
list = "all"
look = "self"
send = "home"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1
autostart = true
groups = ["dev"]

[[sessions]]
id = "alpha"
name = "alpha"
directory = "/tmp"
coder = "tmux-default"

[[sessions]]
id = "bravo"
name = "bravo"
directory = "/tmp"
coder = "acp-broken"
"#,
    )
    .expect("write bundle config");
}

/// Writes a bundle whose only session is an ACP member with a missing binary,
/// so every configured session fails to start and the autostart summary reports
/// the bundle as `failed`.
pub(super) fn write_all_acp_failure_bundle(config_root: &Path, bundle_name: &str) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "acp-broken"

[coders.acp]
channel = "stdio"
command = "/definitely/missing/agentmux-acp"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "all"
look = "self"
send = "home"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1
autostart = true
groups = ["dev"]

[[sessions]]
id = "bravo"
name = "bravo"
directory = "/tmp"
coder = "acp-broken"
"#,
    )
    .expect("write bundle config");
}

pub(super) fn write_bundle_configuration_with_invalid_policy_scope(
    config_root: &Path,
    bundle_name: &str,
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "tmux-default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "home"
look = "self"
send = "home"
choose = "everywhere"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1
autostart = true
groups = ["dev"]

[[sessions]]
id = "alpha"
name = "alpha"
directory = "/tmp"
coder = "tmux-default"
"#,
    )
    .expect("write bundle config");
}

pub(super) fn write_bundle_configuration_with_kill_after_create_member(
    config_root: &Path,
    bundle_name: &str,
) {
    fs::create_dir_all(config_root.join("bundles")).expect("create bundles directory");
    fs::write(
        config_root.join("coders.toml"),
        r#"
format-version = 1

[[coders]]
id = "tmux-default"

[coders.tmux]
initial-command = "sh -lc 'exec sleep 45'"
resume-command = "sh -lc 'exec sleep 45'"
"#,
    )
    .expect("write coders config");
    fs::write(
        config_root.join("policies.toml"),
        r#"
format-version = 1
default = "default"

[[policies]]
id = "default"

[policies.controls]
list = "all"
look = "self"
send = "home"
"#,
    )
    .expect("write policies config");
    fs::write(
        config_root
            .join("bundles")
            .join(format!("{bundle_name}.toml")),
        r#"
format-version = 1
autostart = true

[[sessions]]
id = "steady"
name = "steady"
directory = "/tmp"
coder = "tmux-default"

[[sessions]]
id = "victim"
name = "victim"
directory = "/tmp"
coder = "tmux-default"
"#,
    )
    .expect("write bundle config");
}
