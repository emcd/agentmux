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
    io,
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
};

#[cfg(target_os = "linux")]
use std::{
    ffi::CString,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::PathBuf,
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
///
/// Only Linux takes the descriptor route. Elsewhere there is no `/proc` to
/// address the descriptor through, so opening the parent would buy nothing and
/// is skipped rather than performed and discarded.
#[cfg(target_os = "linux")]
fn with_short_address<T>(
    path: &Path,
    operation: impl FnOnce(&Path) -> io::Result<T>,
) -> io::Result<T> {
    let Some((parent, file_name)) = path.parent().zip(path.file_name()) else {
        return operation(ensure_addressable(path)?);
    };
    // A parent that cannot be opened at all is reported as itself rather than
    // fed through the fallback. There is nothing to bind or connect inside a
    // directory that is not there, and the length check below would otherwise
    // turn a deep-but-absent state root into a path-length fault when the truth
    // is that no relay lives there.
    let directory = open_directory_reference(parent)?;
    let reference = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    // `/proc` is not always mounted, even on Linux. A reference that does not
    // resolve back to the directory just opened is unusable.
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

/// Opens `parent` purely as a location to resolve a name against.
///
/// `O_PATH` rather than a plain read open, because the descriptor is never read
/// from — it exists only to be named through `/proc/self/fd`. A read open would
/// additionally demand read permission on the directory, so a searchable but
/// unreadable parent (`0111`, `0711` to a non-owner) would fail here even
/// though binding and connecting inside it are permitted. That is a real
/// configuration for a peer relay's socket directory, and requiring read there
/// would reject a socket the kernel is willing to serve.
#[cfg(target_os = "linux")]
fn open_directory_reference(parent: &Path) -> io::Result<OwnedFd> {
    let path = CString::new(parent.as_os_str().as_bytes())
        .map_err(|source| io::Error::new(io::ErrorKind::InvalidInput, source))?;
    // SAFETY: `path` is a valid NUL-terminated C string that outlives the call,
    // and the returned descriptor is handed straight to `OwnedFd`, which owns
    // the close.
    let descriptor = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `descriptor` is a fresh, owned, non-negative descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
}

/// Non-Linux targets pass the full path. See the module docs: without `/proc`
/// there is nothing to address a directory descriptor through, and the
/// alternative — changing the working directory — is process-global.
#[cfg(not(target_os = "linux"))]
fn with_short_address<T>(
    path: &Path,
    operation: impl FnOnce(&Path) -> io::Result<T>,
) -> io::Result<T> {
    operation(ensure_addressable(path)?)
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
