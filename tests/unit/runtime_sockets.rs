//! Socket addressing: the length contract, and the permission it must not
//! require.

use std::os::unix::fs::PermissionsExt;

use agentmux::runtime::sockets::{
    UNIX_SOCKET_PATH_MAXIMUM, bind_unix_listener, connect_unix_stream,
};
use tempfile::TempDir;

/// Builds a path under `base` whose length clears `sun_path` by a wide margin.
fn overlong_socket_path(base: &std::path::Path) -> std::path::PathBuf {
    const OVERSHOOT: usize = 60;
    let mut directory = base.to_path_buf();
    while directory.join("relay.sock").as_os_str().len() <= UNIX_SOCKET_PATH_MAXIMUM + OVERSHOOT {
        directory = directory.join("deeply-nested-directory");
    }
    std::fs::create_dir_all(&directory).expect("create deep directory");
    directory.join("relay.sock")
}

/// The limit is per-target because the Darwin fallback passes a real path:
/// `sun_path` is 108 bytes on Linux and 104 on Darwin and the BSDs.
#[test]
fn the_socket_path_limit_matches_the_target() {
    #[cfg(target_os = "linux")]
    assert_eq!(UNIX_SOCKET_PATH_MAXIMUM, 107);
    #[cfg(not(target_os = "linux"))]
    assert_eq!(UNIX_SOCKET_PATH_MAXIMUM, 103);
}

/// Linux addresses the socket through its parent directory, so depth stops
/// mattering. The integration suite proves this end to end by bringing a relay
/// up under such a root; this is the unit-level statement of the same contract.
#[cfg(target_os = "linux")]
#[test]
fn an_overlong_path_binds_and_connects_on_linux() {
    let temporary = TempDir::new().expect("temporary");
    let socket = overlong_socket_path(temporary.path());
    assert!(
        socket.as_os_str().len() > UNIX_SOCKET_PATH_MAXIMUM,
        "fixture must exceed the limit"
    );

    let listener = bind_unix_listener(&socket).expect("bind through the parent directory");
    connect_unix_stream(&socket).expect("connect through the parent directory");
    drop(listener);
}

/// Without `/proc` the full path is what reaches the kernel, so a path over the
/// limit cannot be served. It must still refuse in terms an operator can act
/// on: the limit and the offending path, not a bare `ENAMETOOLONG`.
#[cfg(not(target_os = "linux"))]
#[test]
fn an_overlong_path_is_refused_with_the_limit_named() {
    let temporary = TempDir::new().expect("temporary");
    let socket = overlong_socket_path(temporary.path());

    let error = bind_unix_listener(&socket).expect_err("an overlong path must be refused");
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    let message = error.to_string();
    assert!(
        message.contains(&UNIX_SOCKET_PATH_MAXIMUM.to_string()),
        "the refusal must name the limit: {message}"
    );
    assert!(
        message.contains("relay.sock"),
        "the refusal must name the offending path: {message}"
    );
}

/// A socket directory may be searchable without being readable — `0111`, or
/// `0711` seen by anyone but its owner. Binding and connecting inside such a
/// directory is permitted, so addressing must not quietly demand more than the
/// operation itself does. This is why the parent is opened `O_PATH` rather than
/// for reading.
#[test]
fn a_searchable_but_unreadable_parent_still_serves_its_socket() {
    let temporary = TempDir::new().expect("temporary");
    let directory = temporary.path().join("search-only");
    std::fs::create_dir(&directory).expect("create socket directory");
    let socket = directory.join("relay.sock");

    let listener = bind_unix_listener(&socket).expect("bind before tightening the mode");
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o111))
        .expect("make the directory searchable but unreadable");

    let outcome = connect_unix_stream(&socket);

    // Restored before asserting, so a failure does not leave the temporary
    // directory undeletable.
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
        .expect("restore the directory mode");
    outcome.expect("connect must not require read permission on the parent");
    drop(listener);
}
