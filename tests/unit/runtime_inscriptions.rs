use agentmux::runtime::inscriptions::{mcp_inscriptions_path, relay_inscriptions_path};

#[test]
fn resolves_relay_inscriptions_path_per_bundle() {
    let resolved = relay_inscriptions_path(std::path::Path::new("/inscriptions"), "party-alpha");
    assert_eq!(
        resolved,
        std::path::Path::new("/inscriptions/bundles/party-alpha/relay.log")
    );
}

#[test]
fn resolves_mcp_inscriptions_path_per_bundle_and_session() {
    let resolved = mcp_inscriptions_path(
        std::path::Path::new("/inscriptions"),
        "party-alpha",
        "session-1",
    );
    assert_eq!(
        resolved,
        std::path::Path::new("/inscriptions/bundles/party-alpha/sessions/session-1/mcp.log")
    );
}

#[test]
fn inscriptions_paths_sanitize_unsafe_path_segments() {
    let relay = relay_inscriptions_path(
        std::path::Path::new("/inscriptions"),
        "../party with spaces",
    );
    assert_eq!(
        relay,
        std::path::Path::new("/inscriptions/bundles/.._party_with_spaces/relay.log")
    );

    let mcp = mcp_inscriptions_path(
        std::path::Path::new("/inscriptions"),
        "party",
        "session/with/slashes",
    );
    assert_eq!(
        mcp,
        std::path::Path::new("/inscriptions/bundles/party/sessions/session_with_slashes/mcp.log")
    );
}
