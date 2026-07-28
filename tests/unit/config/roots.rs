use std::path::{Path, PathBuf};

use agentmux::configuration::{ConfigurationRoots, ConfigurationRootsError};

#[test]
fn single_layer_list_is_its_own_base() {
    let roots = ConfigurationRoots::single("/base");
    assert_eq!(roots.layers(), [PathBuf::from("/base")]);
    assert_eq!(roots.base_layer(), Path::new("/base"));
}

#[test]
fn elements_keep_the_order_they_were_supplied_in() {
    let roots = ConfigurationRoots::from_elements(["/rnd", "/shared", "/base"].map(PathBuf::from))
        .expect("list should build");

    // First-wins: the first element is the override, the last is the base.
    assert_eq!(
        roots.layers(),
        ["/rnd", "/shared", "/base"].map(PathBuf::from)
    );
    assert_eq!(roots.base_layer(), Path::new("/base"));
}

#[test]
fn an_empty_element_is_rejected_rather_than_read_as_the_working_directory() {
    let error = ConfigurationRoots::from_elements(["/base", ""].map(PathBuf::from))
        .expect_err("an empty element must not contribute a layer");
    assert_eq!(error, ConfigurationRootsError::EmptyElement { position: 1 });
}

#[test]
fn an_empty_list_is_rejected() {
    let error =
        ConfigurationRoots::from_elements(Vec::new()).expect_err("an empty list must not build");
    assert_eq!(error, ConfigurationRootsError::EmptyList);
}

#[test]
fn environment_value_splits_on_the_separator_in_order() {
    let roots = ConfigurationRoots::from_environment_value("/rnd:/base")
        .expect("value should parse")
        .expect("value should supply a list");
    assert_eq!(roots.layers(), ["/rnd", "/base"].map(PathBuf::from));
}

#[test]
fn environment_value_carrying_only_whitespace_leaves_the_variable_unset() {
    // Falling through to the tier below, rather than contributing a layer named
    // by whitespace.
    assert_eq!(
        ConfigurationRoots::from_environment_value("   ").expect("value should parse"),
        None
    );
}

#[test]
fn a_separator_contributing_an_empty_element_is_rejected() {
    for (value, position) in [(":/base", 0), ("/base:", 1), ("/rnd::/base", 1)] {
        let error = ConfigurationRoots::from_environment_value(value)
            .expect_err("an empty element must not contribute a layer");
        assert_eq!(
            error,
            ConfigurationRootsError::EmptyElement { position },
            "unexpected error for {value}"
        );
    }
}

#[test]
fn a_relative_element_is_preserved_for_the_caller_to_resolve() {
    // Resolution against the working directory is the caller's job; the list
    // does not silently absolutize, which would hide which form was supplied.
    let roots = ConfigurationRoots::from_environment_value("relative/layer:/base")
        .expect("value should parse")
        .expect("value should supply a list");
    assert_eq!(
        roots.layers(),
        ["relative/layer", "/base"].map(PathBuf::from)
    );
}
