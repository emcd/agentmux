use std::{fs, path::Path};

use agentmux::configuration::ConfigurationRoots;
use agentmux::runtime::{
    paths::{ConfigurationRootSource, RuntimeRoots},
    starter::ensure_starter_configuration_layout,
};
use tempfile::TempDir;

/// Roots naming `configuration_root`, presented as resolved from the default
/// tier so hydration applies. A defaulted list is always a single layer, which
/// is the only shape hydration ever sees.
fn defaulted_roots(configuration_root: &Path) -> RuntimeRoots {
    roots_from(configuration_root, ConfigurationRootSource::Default)
}

fn roots_from(configuration_root: &Path, source: ConfigurationRootSource) -> RuntimeRoots {
    RuntimeRoots {
        configuration_roots: ConfigurationRoots::single(configuration_root),
        state_root: configuration_root.join("state"),
        inscriptions_root: configuration_root.join("inscriptions"),
        configuration_root_source: source,
    }
}

#[test]
fn creates_starter_configuration_files_when_missing() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("config");

    ensure_starter_configuration_layout(&defaulted_roots(&configuration_root))
        .expect("starter layout");

    let coders = configuration_root.join("coders.toml");
    let policies = configuration_root.join("policies.toml");
    let users = configuration_root.join("users.toml");
    let ui = configuration_root.join("ui.toml");
    let relay = configuration_root.join("relay.toml");
    let example_bundle = configuration_root.join("bundles/example.toml");
    assert!(coders.exists(), "expected coders.toml to exist");
    assert!(policies.exists(), "expected policies.toml to exist");
    assert!(users.exists(), "expected users.toml to exist");
    assert!(ui.exists(), "expected ui.toml to exist");
    assert!(relay.exists(), "expected relay.toml to exist");
    assert!(example_bundle.exists(), "expected example bundle to exist");

    let coders_text = fs::read_to_string(coders).expect("read coders.toml");
    assert!(coders_text.contains("format-version = 1"));
    assert!(coders_text.contains("[[coders]]"));
    let policies_text = fs::read_to_string(policies).expect("read policies.toml");
    assert!(policies_text.contains("format-version = 1"));
    assert!(policies_text.contains("[[policies]]"));
    let users_text = fs::read_to_string(users).expect("read users.toml");
    assert!(users_text.contains("default-session"));
    assert!(users_text.contains("[[sessions]]"));
    // ui.toml carries the UI-surface defaults (a commented default-bundle
    // example); it is fully commented, so it activates no key.
    let ui_text = fs::read_to_string(ui).expect("read ui.toml");
    assert!(ui_text.contains("default-bundle"));
    assert!(
        ui_text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .all(|line| line.trim().is_empty()),
        "ui.toml template must have no active (uncommented) keys",
    );
    // The relay.toml template is fully commented (all-defaults), so it must
    // contain no active key that could change relay behavior — only documentation.
    let relay_text = fs::read_to_string(relay).expect("read relay.toml");
    assert!(relay_text.contains("watch-bundles"));
    assert!(relay_text.contains("[[peers]]"));
    assert!(
        relay_text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .all(|line| line.trim().is_empty()),
        "relay.toml template must have no active (uncommented) keys",
    );

    let bundle_text = fs::read_to_string(example_bundle).expect("read example bundle");
    assert!(bundle_text.contains("format-version = 1"));
    assert!(bundle_text.contains("[[sessions]]"));
}

#[test]
fn preserves_existing_configuration_files() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("config");
    fs::create_dir_all(configuration_root.join("bundles")).expect("create bundle dir");
    let coders = configuration_root.join("coders.toml");
    let policies = configuration_root.join("policies.toml");
    let users = configuration_root.join("users.toml");
    let relay = configuration_root.join("relay.toml");
    let example_bundle = configuration_root.join("bundles/example.toml");
    fs::write(&coders, "format-version = 1\n# custom coders\n").expect("write coders");
    fs::write(&policies, "format-version = 1\n# custom policies\n").expect("write policies");
    fs::write(
        &users,
        "default-session = \"custom@GLOBAL\"\n# custom users\n",
    )
    .expect("write users");
    fs::write(&relay, "watch-bundles = false\n# custom relay\n").expect("write relay");
    fs::write(&example_bundle, "format-version = 1\n# custom bundle\n").expect("write bundle");

    ensure_starter_configuration_layout(&defaulted_roots(&configuration_root))
        .expect("starter layout");

    let coders_text = fs::read_to_string(coders).expect("read coders.toml");
    assert_eq!(coders_text, "format-version = 1\n# custom coders\n");
    let policies_text = fs::read_to_string(policies).expect("read policies.toml");
    assert_eq!(policies_text, "format-version = 1\n# custom policies\n");
    let users_text = fs::read_to_string(users).expect("read users.toml");
    assert_eq!(
        users_text,
        "default-session = \"custom@GLOBAL\"\n# custom users\n"
    );
    let relay_text = fs::read_to_string(relay).expect("read relay.toml");
    assert_eq!(relay_text, "watch-bundles = false\n# custom relay\n");
    let bundle_text = fs::read_to_string(example_bundle).expect("read example bundle");
    assert_eq!(bundle_text, "format-version = 1\n# custom bundle\n");
}

#[test]
fn skips_example_seed_when_bundles_directory_already_has_toml() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("config");
    fs::create_dir_all(configuration_root.join("bundles")).expect("create bundle dir");
    let operator_bundle = configuration_root.join("bundles/production.toml");
    fs::write(&operator_bundle, "format-version = 1\n# operator bundle\n").expect("write bundle");

    ensure_starter_configuration_layout(&defaulted_roots(&configuration_root))
        .expect("starter layout");

    let example_bundle = configuration_root.join("bundles/example.toml");
    assert!(
        !example_bundle.exists(),
        "expected example.toml to stay unseeded when a real bundle is present"
    );
    let operator_text = fs::read_to_string(&operator_bundle).expect("read operator bundle");
    assert_eq!(operator_text, "format-version = 1\n# operator bundle\n");
}

#[test]
fn refuses_to_scaffold_an_explicitly_named_root() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("named-but-absent");

    for source in [
        ConfigurationRootSource::CommandLine,
        ConfigurationRootSource::Environment,
    ] {
        let error = ensure_starter_configuration_layout(&roots_from(&configuration_root, source))
            .expect_err("naming an absent root should fault rather than scaffold");
        assert!(
            error
                .to_string()
                .contains("configuration layer does not exist"),
            "unexpected error for {source:?}: {error}"
        );
        assert!(
            !configuration_root.exists(),
            "{source:?} root must not be created"
        );
    }
}

#[test]
fn leaves_an_existing_explicitly_named_root_unscaffolded() {
    let temporary = TempDir::new().expect("temporary");
    let configuration_root = temporary.path().join("config");
    fs::create_dir_all(&configuration_root).expect("create configuration root");

    ensure_starter_configuration_layout(&roots_from(
        &configuration_root,
        ConfigurationRootSource::CommandLine,
    ))
    .expect("existing named root is accepted");

    assert!(
        !configuration_root.join("coders.toml").exists(),
        "a named root must never gain starter files"
    );
    assert!(
        !configuration_root.join("bundles").exists(),
        "a named root must never gain a bundles directory"
    );
}
