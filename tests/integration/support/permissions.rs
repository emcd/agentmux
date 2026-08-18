//! Filesystem-permission fixtures for the tests that need a directory the
//! process genuinely cannot read.
//!
//! The whole point of these fixtures is to make a read *fail*. Running as root,
//! holding `CAP_DAC_OVERRIDE`, or sitting on a filesystem that does not enforce
//! modes all leave the read succeeding, and a test that then asserts a fault
//! would report a pass having never reached the path it was written for. So the
//! fixture is verified rather than assumed: the mode is applied and the read is
//! attempted, and a read that succeeds means no fixture.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

/// Restores a directory's original mode when dropped.
///
/// Without this a failed assertion leaves a mode-0 directory inside the
/// temporary tree, which the `TempDir` destructor then cannot remove — turning
/// one failing test into a leaked directory and a confusing second error.
/// Restoration runs on the unwinding path precisely because that is the case
/// that matters.
pub(crate) struct RestoredMode {
    directory: PathBuf,
    original: u32,
}

impl Drop for RestoredMode {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.directory, fs::Permissions::from_mode(self.original));
    }
}

/// Makes `directory` unreadable, returning the restoring guard, or `None` when
/// the fixture does not bite in this environment.
///
/// A `None` return is the caller's signal to report a skip and stop. It is not
/// a failure: nothing is wrong with the code under test, the environment simply
/// cannot express the condition being tested.
pub(crate) fn deny_directory_access(directory: &Path) -> Option<RestoredMode> {
    let original = fs::metadata(directory)
        .expect("read directory mode")
        .permissions()
        .mode();
    fs::set_permissions(directory, fs::Permissions::from_mode(0o000))
        .expect("apply the unreadable mode");
    let guard = RestoredMode {
        directory: directory.to_path_buf(),
        original,
    };
    // The verification, not a sanity check: this is what distinguishes a fixture
    // from a decoration.
    if fs::read_dir(directory).is_ok() {
        return None;
    }
    Some(guard)
}

/// Reports that a test could not run, naming why, so a skip is visible in the
/// run rather than indistinguishable from a pass.
pub(crate) fn report_permission_fixture_skip(test_name: &str) {
    eprintln!(
        "SKIP {test_name}: a mode-0 directory is still readable by this process \
         (running as root, holding CAP_DAC_OVERRIDE, or on a filesystem that does \
         not enforce modes), so the unreadable-layer path cannot be exercised here"
    );
}
