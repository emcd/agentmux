//! Unix-domain socket addressing that does not scale with state-root depth.
//!
//! `sockaddr_un.sun_path` is 108 bytes on Linux, 107 usable. The deepest path
//! this project constructs — `<state_root>/bundles/<bundle>/tmux.sock` — already
//! measures 96 bytes for a repository-local root in a worktree checkout, so a
//! slightly longer checkout or bundle name overflows it. Deployments that name
//! an explicit state root are exactly the deep ones.
//!
//! A relative path used to be the escape hatch: it produced a short string to
//! bind against. State-root normalization removed that, so the short string is
//! reconstructed here instead. The two requirements are separable — the state
//! root must be absolute so a spawned child resolves the same directory
//! whatever its working directory, while the string handed to `bind` or
//! `connect` is a different string and nothing requires it to be the same one.
//!
//! The parent directory is opened and the socket addressed through that
//! descriptor as `/proc/self/fd/<n>/<name>`, which is bounded at roughly 30
//! bytes however deep the real directory is. The directory is already `0700`
//! and the descriptor is the process's own, so this adds no security surface.
//! `/proc` is Linux-only; the full path is used where the descriptor form is
//! unavailable, and the length is checked first so the failure names the limit
//! rather than arriving as a bare `ENAMETOOLONG`.

use std::{
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

/// Usable bytes in `sockaddr_un.sun_path`: 108 on Linux, less the NUL
/// terminator.
pub const UNIX_SOCKET_PATH_MAXIMUM: usize = 107;

/// Binds a Unix listener at `path`, addressing it through its parent directory.
///
/// # Errors
///
/// Returns the underlying bind error, or an `InvalidInput` error naming the
/// limit when no addressable form of the path fits in `sun_path`.
pub fn bind_unix_listener(path: &Path) -> io::Result<UnixListener> {
    with_short_address(path, |address| UnixListener::bind(address))
}

/// Connects to a Unix socket at `path`, addressing it through its parent
/// directory.
///
/// # Errors
///
/// Returns the underlying connect error, or an `InvalidInput` error naming the
/// limit when no addressable form of the path fits in `sun_path`.
pub fn connect_unix_stream(path: &Path) -> io::Result<UnixStream> {
    with_short_address(path, |address| UnixStream::connect(address))
}

/// Resolves `path` to an address that fits `sun_path` and applies `operation`
/// to it.
fn with_short_address<T>(
    path: &Path,
    operation: impl FnOnce(&Path) -> io::Result<T>,
) -> io::Result<T> {
    let short = path
        .parent()
        .zip(path.file_name())
        .and_then(|(parent, file_name)| {
            let directory = File::open(parent).ok()?;
            let reference = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
            // `/proc` is not mounted everywhere. A reference that does not
            // resolve back to the directory just opened is unusable.
            if !reference.is_dir() {
                return None;
            }
            Some((directory, reference.join(file_name)))
        });
    let Some((directory, address)) = short else {
        return operation(ensure_addressable(path)?);
    };
    // `directory` is held across the call: the `/proc/self/fd/<n>` component is
    // only meaningful while the descriptor is live, and dropping it before the
    // syscall would leave the address dangling.
    let outcome = operation(ensure_addressable(&address)?);
    drop(directory);
    outcome
}

/// Rejects an address that cannot fit in `sun_path` before the kernel does, so
/// the failure names the limit and the offending path instead of surfacing as a
/// bare `ENAMETOOLONG` an operator cannot act on.
fn ensure_addressable(path: &Path) -> io::Result<&Path> {
    let length = path.as_os_str().len();
    if length <= UNIX_SOCKET_PATH_MAXIMUM {
        return Ok(path);
    }
    Err(io::Error::new(
        io::ErrorKind::InvalidInput,
        format!(
            "unix socket path is {length} bytes, over the {UNIX_SOCKET_PATH_MAXIMUM}-byte \
             sun_path limit: {}",
            path.display()
        ),
    ))
}
