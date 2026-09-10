//! No-follow confined filesystem writes anchored at the state root.
//!
//! Every state-root-owned credential write (credential-sink staging, peer-slot
//! installation, principal-store load/persist) traverses path components
//! without following symlinks and performs all post-staging operations
//! relative to retained directory handles, so an ancestor exchanged for a
//! symlink between staging and commit still aborts. Caller-named `path`
//! sinks are excluded: their pre-existing parents live outside the state
//! root trust anchor and keep the final-target symlink check only.

use std::{
    ffi::CString,
    fs, io,
    os::unix::{
        ffi::OsStrExt,
        fs::PermissionsExt,
        io::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    },
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Owner-only mode for created credential directories and credential files.
pub(crate) const CONFINED_DIR_MODE: u32 = 0o700;
pub(crate) const CONFINED_FILE_MODE: u32 = 0o600;

/// Process-unique nonce source for temp-file names, so a staged credential or
/// store temp never collides with a concurrent or stale artifact.
static TEMP_FILE_NONCE: AtomicU64 = AtomicU64::new(0);

/// Builds a per-attempt-unique sibling temp path for `final_path`. Combining
/// the pid with a monotonic nonce keeps two concurrent writers (and any stale
/// crash-left artifact) from ever selecting the same temp name, so a
/// `create_new` open can safely refuse a pre-existing file.
pub(crate) fn unique_temp_path(final_path: &Path, tag: &str) -> PathBuf {
    let nonce = TEMP_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut name = final_path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{pid}.{nonce}.{tag}.tmp"));
    final_path.with_file_name(name)
}

/// A confined-traversal failure: either a symlinked ancestor component (the
/// caller maps it to `validation_invalid_credential_path`), a post-rename
/// directory-sync failure (the caller maps it to its durability-uncertain
/// outcome), or an I/O failure the caller maps to its own internal error
/// code.
#[derive(Debug)]
pub(crate) enum ConfineError {
    Symlink { component: String },
    Exchanged { component: String },
    DirSync { source: io::Error },
    Io { source: io::Error },
}

impl ConfineError {
    fn symlink(component: &str) -> Self {
        Self::Symlink {
            component: component.to_string(),
        }
    }

    fn io(source: io::Error) -> Self {
        Self::Io { source }
    }

    /// Returns the I/O failure for non-symlink errors. Symlink errors carry
    /// no I/O failure; callers handle them first.
    pub(crate) fn io_source(self) -> Option<io::Error> {
        match self {
            Self::Symlink { .. } | Self::Exchanged { .. } => None,
            Self::DirSync { source } => Some(source),
            Self::Io { source } => Some(source),
        }
    }
}

/// Opens `state_root` itself without following a trailing symlink, returning
/// the anchor handle every confined traversal starts from. A missing root
/// resolves to `None` when `create` is false (absent path, not an error);
/// when `create` holds, the root is created first, mirroring the previous
/// `create_dir_all` behavior for a not-yet-existing state tree.
fn open_confined_root(state_root: &Path, create: bool) -> Result<Option<OwnedFd>, ConfineError> {
    let path = cstring_from_path(state_root).ok_or_else(|| {
        ConfineError::io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "state root is not a path",
        ))
    })?;
    match open_at(libc::AT_FDCWD, &path, open_dir_flags(), 0) {
        // `O_NOFOLLOW | O_DIRECTORY` refuses a symlinked state root rather
        // than anchoring the traversal outside the tree.
        Ok(fd) => Ok(Some(fd)),
        Err(source) if source.kind() == io::ErrorKind::NotFound && !create => Ok(None),
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(state_root).map_err(ConfineError::io)?;
            open_at(libc::AT_FDCWD, &path, open_dir_flags(), 0)
                .map(Some)
                .map_err(|source| map_dir_open_error(libc::AT_FDCWD, &path, source))
        }
        Err(source) => Err(map_dir_open_error(libc::AT_FDCWD, &path, source)),
    }
}

/// Walks `components` below `parent`, refusing symlinked components, and
/// returns the final directory handle. Missing components are created with
/// owner-only mode when `create` holds; otherwise a missing component
/// resolves to `None` (absent path, not an error).
fn walk_confined(
    parent: BorrowedFd,
    components: &[&Path],
    create: bool,
) -> Result<Option<OwnedFd>, ConfineError> {
    let mut current = parent.try_clone_to_owned().map_err(ConfineError::io)?;
    for component in components {
        let raw = component.as_os_str().as_bytes();
        check_component_name(raw, component)?;
        let cname = CString::new(raw).map_err(|_| {
            ConfineError::io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path component holds NUL",
            ))
        })?;
        match open_at(current.as_raw_fd(), &cname, open_dir_flags(), 0) {
            Ok(child) => current = child,
            Err(source) if source.kind() == io::ErrorKind::NotFound && create => {
                // `mkdirat` derives the directory from the retained parent
                // handle, so a concurrently exchanged ancestor cannot redirect
                // the creation.
                let created = unsafe {
                    libc::mkdirat(current.as_raw_fd(), cname.as_ptr(), CONFINED_DIR_MODE)
                };
                if created != 0 {
                    return Err(map_open_error(
                        io::Error::last_os_error(),
                        &component.display().to_string(),
                    ));
                }
                let reopen_fd = current.as_raw_fd();
                current = open_at(reopen_fd, &cname, open_dir_flags(), 0)
                    .map_err(|source| map_dir_open_error(reopen_fd, &cname, source))?;
            }
            Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(source) => {
                let parent_fd = current.as_raw_fd();
                return Err(map_dir_open_error(parent_fd, &cname, source));
            }
        }
    }
    Ok(Some(current))
}

/// Splits a state-root-relative path into parent components plus the file
/// name, rejecting absolute paths and parent-directory escapes: confined
/// targets are always constructed below the anchor, never addressed through
/// it.
fn split_confined_relative(relative: &Path) -> Result<(Vec<&Path>, &Path), ConfineError> {
    if relative.is_absolute() {
        return Err(ConfineError::io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "confined target must be relative",
        )));
    }
    let mut parents = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(name) => parents.push(Path::new(name)),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(ConfineError::io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "confined target escapes anchor",
                )));
            }
            Component::CurDir => {}
        }
    }
    let Some(file_name) = parents.pop() else {
        return Err(ConfineError::io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "confined target has no file name",
        )));
    };
    Ok((parents, file_name))
}

/// Stages `bytes` for the state-root-relative `target`: walks (creating)
/// confined parents, writes a fresh 0600 sibling temp with fsync, and
/// returns the handle whose commit publishes it. Nothing is published yet.
pub(crate) fn confined_stage(
    state_root: &Path,
    target: &Path,
    bytes: &[u8],
    tag: &str,
) -> Result<ConfinedWrite, ConfineError> {
    let (parents, file_name) = split_confined_relative(target)?;
    let Some(root) = open_confined_root(state_root, true)? else {
        return Err(ConfineError::io(io::Error::new(
            io::ErrorKind::NotFound,
            "confined root vanished",
        )));
    };
    let Some(parent) = walk_confined(root.as_fd(), &parents, true)? else {
        return Err(ConfineError::io(io::Error::new(
            io::ErrorKind::NotFound,
            "confined parent vanished",
        )));
    };
    // Test fault-injection seam for the pre-rename path: a
    // `.fault-pre-rename` file beside the target fails staging after the
    // temp is written but before publication, so tests assert the
    // old-content-intact contract deterministically.
    let tmp_name = temp_file_name(state_root, target, tag);
    let file = open_at(
        parent.as_raw_fd(),
        &tmp_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        CONFINED_FILE_MODE,
    )
    .map_err(|source| map_open_error(source, &target.display().to_string()))?;
    let mut file = fs::File::from(file);
    // Enforce exactly 0600 before the secret lands: `openat` applied the mode
    // under the process umask, so reassert it on the retained handle.
    file.set_permissions(fs::Permissions::from_mode(CONFINED_FILE_MODE))
        .map_err(ConfineError::io)?;
    if let Err(source) = io::Write::write_all(&mut file, bytes).and_then(|()| file.sync_all()) {
        remove_relative(&parent, &tmp_name);
        return Err(ConfineError::io(source));
    }
    if fault_seam_present(&parent, ".fault-pre-rename") {
        remove_relative(&parent, &tmp_name);
        return Err(ConfineError::io(io::Error::other(
            "injected fault: .fault-pre-rename present",
        )));
    }
    Ok(ConfinedWrite {
        parent,
        tmp_name,
        final_name: cname_of(file_name),
        display_path: state_root.join(target).display().to_string(),
        anchor: state_root.to_path_buf(),
        parents: parents.iter().collect(),
    })
}

/// Reads the state-root-relative `target` through confined traversal,
/// returning `None` when the file (or any parent) is absent. A symlinked
/// ancestor aborts with a symlink error rather than resolving outside the
/// state tree.
pub(crate) fn confined_read(
    state_root: &Path,
    target: &Path,
) -> Result<Option<Vec<u8>>, ConfineError> {
    let (parents, file_name) = split_confined_relative(target)?;
    let Some(root) = open_confined_root(state_root, false)? else {
        return Ok(None);
    };
    let Some(parent) = walk_confined(root.as_fd(), &parents, false)? else {
        return Ok(None);
    };
    let final_name = cname_of(file_name);
    let file = match open_at(parent.as_raw_fd(), &final_name, open_file_flags(), 0) {
        Ok(file) => fs::File::from(file),
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(map_open_error(source, &target.display().to_string()));
        }
    };
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut io::BufReader::new(file), &mut bytes).map_err(ConfineError::io)?;
    Ok(Some(bytes))
}

/// A staged confined write awaiting atomic publication.
pub(crate) struct ConfinedWrite {
    parent: OwnedFd,
    tmp_name: CString,
    final_name: CString,
    display_path: String,
    /// Anchor and relative parent for commit-time re-verification.
    anchor: PathBuf,
    parents: PathBuf,
}

impl ConfinedWrite {
    /// Returns the absolute display path the commit will report. Lexical
    /// only: no filesystem access, so retained-handle confinement is
    /// unaffected.
    pub(crate) fn display_path(&self) -> &str {
        self.display_path.as_str()
    }

    /// Publishes the staged file with an atomic rename executed against the
    /// retained parent handle, then syncs that handle so the rename is
    /// durable before success is reported. The test exchange hook fires
    /// between staging and rename, so an ancestor swapped for a symlink
    /// after staging still aborts: every operation below addresses the
    /// retained handle, never a re-walked pathname.
    ///
    /// A directory-sync failure after the rename reports the failure without
    /// rolling back: publication already occurred, so the caller maps it to
    /// its durability-uncertain outcome.
    pub(crate) fn commit(self) -> Result<String, ConfineError> {
        super::test_hooks::fire_confined_commit_hook();
        // Re-verify the anchor chain before publishing: the rename below is
        // fully handle-relative, so an exchanged ancestor cannot redirect
        // it — but publishing into a detached directory would still be
        // wrong. Re-walking and comparing directory identity turns any
        // staged-to-commit exchange into a loud abort instead.
        if let Err(error) = self.verify_anchor() {
            self.abort();
            return Err(error);
        }
        let renamed = unsafe {
            libc::renameat(
                self.parent.as_raw_fd(),
                self.tmp_name.as_ptr(),
                self.parent.as_raw_fd(),
                self.final_name.as_ptr(),
            )
        };
        if renamed != 0 {
            let source = io::Error::last_os_error();
            remove_relative(&self.parent, &self.tmp_name);
            return Err(ConfineError::io(source));
        }
        if fault_seam_present(&self.parent, ".fault-dir-sync") {
            return Err(ConfineError::DirSync {
                source: io::Error::other("injected fault: .fault-dir-sync present"),
            });
        }
        sync_fd(&self.parent).map_err(|source| ConfineError::DirSync { source })?;
        Ok(self.display_path)
    }

    /// Discards the staged temp file without publishing it.
    pub(crate) fn abort(self) {
        remove_relative(&self.parent, &self.tmp_name);
    }

    /// Re-walks the anchor chain by pathname and compares the re-resolved
    /// parent directory identity against the retained handle. A symlinked
    /// component aborts with its name; a silently exchanged (or vanished)
    /// ancestor aborts rather than publishing into a detached directory.
    fn verify_anchor(&self) -> Result<(), ConfineError> {
        let components: Vec<&Path> = self
            .parents
            .components()
            .map(|component| Path::new(component.as_os_str()))
            .collect();
        let Some(root) = open_confined_root(&self.anchor, false)? else {
            return Err(ConfineError::io(io::Error::new(
                io::ErrorKind::NotFound,
                "confined anchor vanished before commit",
            )));
        };
        let Some(fresh) = walk_confined(root.as_fd(), &components, false)? else {
            return Err(ConfineError::io(io::Error::new(
                io::ErrorKind::NotFound,
                "confined parent vanished before commit",
            )));
        };
        let (held_dev, held_ino) = fd_identity(self.parent.as_fd())?;
        let (fresh_dev, fresh_ino) = fd_identity(fresh.as_fd())?;
        if (held_dev, held_ino) != (fresh_dev, fresh_ino) {
            return Err(ConfineError::Exchanged {
                component: self.parents.display().to_string(),
            });
        }
        Ok(())
    }
}

/// Returns the device/inode identity of an open directory handle.
fn fd_identity(fd: BorrowedFd) -> Result<(u64, u64), ConfineError> {
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut status) } != 0 {
        return Err(ConfineError::io(io::Error::last_os_error()));
    }
    Ok((status.st_dev as u64, status.st_ino as u64))
}

/// Derives the sibling temp file name for `target` without touching the
/// filesystem: only the file-name mapping of [`unique_temp_path`] is used.
fn temp_file_name(state_root: &Path, target: &Path, tag: &str) -> CString {
    let tmp = unique_temp_path(&state_root.join(target), tag);
    cname_of(tmp.file_name().map(Path::new).unwrap_or(target))
}

/// Converts a path component to a `CString`, rejecting interior NUL bytes
/// that cannot name a filesystem entry.
fn cname_of(component: &Path) -> CString {
    CString::new(component.as_os_str().as_bytes()).expect("confined target component holds NUL")
}

/// Converts a whole path to a `CString` for the anchor open, returning
/// `None` on interior NUL bytes.
fn cstring_from_path(path: &Path) -> Option<CString> {
    CString::new(path.as_os_str().as_bytes()).ok()
}

/// Flags for opening a directory without following a trailing symlink.
fn open_dir_flags() -> libc::c_int {
    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

/// Flags for opening a regular file without following a trailing symlink.
fn open_file_flags() -> libc::c_int {
    libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC
}

/// Opens `name` relative to the `parent` directory handle.
///
/// # Safety
///
/// The caller supplies a valid open directory handle and a NUL-terminated
/// name; the call performs no other memory access.
fn open_at(parent: RawFd, name: &CString, flags: libc::c_int, mode: u32) -> io::Result<OwnedFd> {
    let fd = unsafe { libc::openat(parent, name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // `openat` returned ownership of a fresh descriptor on success.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Maps an `openat`/`mkdirat` failure to a confinement error: a symlink
/// refusal names the offending component for
/// `validation_invalid_credential_path`; every other failure keeps its I/O
/// identity for the caller's internal error mapping.
///
/// A no-follow directory open reports a symlink-to-directory as `ENOTDIR`
/// rather than `ELOOP` on Linux, so directory opens disambiguate with an
/// `lstat`: symlink means refusal, anything else keeps the I/O error.
fn map_open_error(source: io::Error, component: &str) -> ConfineError {
    if source.raw_os_error() == Some(libc::ELOOP) {
        return ConfineError::symlink(component);
    }
    ConfineError::io(source)
}

/// Maps a no-follow *directory* open failure, disambiguating the `ENOTDIR`
/// a symlink-to-directory produces from a genuine non-directory.
fn map_dir_open_error(parent: RawFd, name: &CString, source: io::Error) -> ConfineError {
    if source.raw_os_error() == Some(libc::ELOOP) {
        let component = name.to_string_lossy().into_owned();
        return ConfineError::symlink(component.as_str());
    }
    if source.raw_os_error() == Some(libc::ENOTDIR) && is_symlink_at(parent, name) {
        let component = name.to_string_lossy().into_owned();
        return ConfineError::symlink(component.as_str());
    }
    ConfineError::io(source)
}

/// Reports whether `name` relative to the `parent` handle is a symlink,
/// without following it.
fn is_symlink_at(parent: RawFd, name: &CString) -> bool {
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::fstatat(
            parent,
            name.as_ptr(),
            &mut status,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return false;
    }
    status.st_mode & libc::S_IFMT == libc::S_IFLNK
}

/// Best-effort removal of a sibling temp file relative to its parent handle.
fn remove_relative(parent: &OwnedFd, name: &CString) {
    unsafe {
        libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0);
    }
}

/// Reports whether a fault-injection sentinel file is present beside the
/// staged target, checked relative to the retained parent handle.
fn fault_seam_present(parent: &OwnedFd, seam: &str) -> bool {
    let Ok(name) = CString::new(seam) else {
        return false;
    };
    unsafe { libc::faccessat(parent.as_raw_fd(), name.as_ptr(), libc::F_OK, 0) == 0 }
}

/// Syncs a retained directory handle so a just-renamed entry is durable.
fn sync_fd(dir: &OwnedFd) -> io::Result<()> {
    fs::File::from(dir.try_clone()?).sync_all()
}

/// Rejects the `.` / `..` / empty segments that must never appear in a
/// constructed confined target.
fn check_component_name(raw: &[u8], component: &Path) -> Result<(), ConfineError> {
    if raw.is_empty() || raw == b"." || raw == b".." {
        return Err(ConfineError::io(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "confined target holds an unsafe component: {}",
                component.display()
            ),
        )));
    }
    Ok(())
}
