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
//!
//! # Portability
//!
//! `/proc` is Linux-only, and the descriptor form is used only where it
//! resolves. On Darwin the full path is passed instead, which is what every
//! caller did before this module existed — macOS keeps today's reach and does
//! not gain the depth-independence. Closing that gap needs a working-directory
//! change, which is process-global and therefore not worth taking where the
//! descriptor form is available; `bindat`/`connectat` would serve but Darwin
//! does not provide them.
//!
//! Because the fallback is a real path on a real platform, the limit is
//! per-target rather than Linux's number everywhere: a Darwin path between the
//! two limits would otherwise pass the check and then fail with the bare
//! `ENAMETOOLONG` the check exists to replace.
//!
//! Windows is an intended target — the Pty transport exists in part to give it
//! a path that does not go through tmux — but this module cannot serve it yet.
//! It is written against `std::os::unix::net`, and `std` exposes no AF_UNIX
//! types on Windows even though the OS has supported the family since Windows
//! 10; reaching it needs a third-party implementation. When that lands, this is
//! one of the places needing a target-specific arm: there is no `/proc`, so the
//! descriptor form does not carry over and the full path would be used, and the
//! per-target limit below needs a Windows value (its `sun_path` is 108 bytes,
//! matching Linux rather than the Darwin figure the fallback arm currently
//! carries).

use std::{
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

/// Usable bytes in `sockaddr_un.sun_path`, less the NUL terminator.
///
/// The field is 108 bytes on Linux and 104 on Darwin and the BSDs, whose
/// `sockaddr_un` also carries a leading `sun_len`. Reporting Linux's number on
/// Darwin would admit paths the kernel then rejects.
#[cfg(target_os = "linux")]
pub const UNIX_SOCKET_PATH_MAXIMUM: usize = 107;

/// Usable bytes in `sockaddr_un.sun_path`, less the NUL terminator. See the
/// Linux definition above for why this is per-target.
///
/// This arm carries the Darwin/BSD figure, which is the only non-Linux target
/// the module compiles for today. A Windows arm needs 107, not this value.
#[cfg(not(target_os = "linux"))]
pub const UNIX_SOCKET_PATH_MAXIMUM: usize = 103;

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
    let Some((parent, file_name)) = path.parent().zip(path.file_name()) else {
        return operation(ensure_addressable(path)?);
    };
    // A parent that cannot be opened is reported as itself rather than fed
    // through the fallback. There is nothing to bind or connect inside a
    // directory that is not there, and the length check below would otherwise
    // convert a deep-but-absent state root into a path-length fault when the
    // truth is that no relay lives there.
    let directory = File::open(parent)?;
    let reference = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    // `/proc` is not mounted everywhere. A reference that does not resolve back
    // to the directory just opened is unusable.
    if !reference.is_dir() {
        return operation(ensure_addressable(path)?);
    }
    // `directory` is held across the call: the `/proc/self/fd/<n>` component is
    // only meaningful while the descriptor is live, and dropping it before the
    // syscall would leave the address dangling.
    let outcome = operation(ensure_addressable(&reference.join(file_name))?);
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
