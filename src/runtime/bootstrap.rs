//! Relay bootstrap coordination primitives for client startup.

use std::{
    fs::{self, OpenOptions},
    io,
    os::fd::AsRawFd,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use super::{
    error::RuntimeError,
    paths::{RelayRuntimePaths, ensure_existing_artifact_is_owned, ensure_relay_runtime_directory},
};

const SOCKET_MODE_OWNER_ONLY: u32 = 0o600;
const POLL_SLEEP_INTERVAL: Duration = Duration::from_millis(50);

/// Client-side bootstrap options for relay startup behavior.
#[derive(Clone, Copy, Debug)]
pub struct BootstrapOptions {
    pub auto_start_relay: bool,
    pub startup_timeout: Duration,
}

impl Default for BootstrapOptions {
    fn default() -> Self {
        Self {
            auto_start_relay: true,
            startup_timeout: Duration::from_millis(10_000),
        }
    }
}

/// Outcome of relay bootstrap.
#[derive(Clone, Debug)]
pub struct BootstrapReport {
    pub spawned_relay: bool,
}

/// Acquires and holds the relay runtime lock.
#[derive(Debug)]
pub struct RelayRuntimeLock {
    lock_file: std::fs::File,
}

impl Drop for RelayRuntimeLock {
    fn drop(&mut self) {
        unlock(&self.lock_file);
        let _ = self.lock_file.set_len(0);
        let _ = self.lock_file.sync_all();
    }
}

#[derive(Debug)]
struct SpawnLockGuard {
    lock_file: std::fs::File,
}

impl Drop for SpawnLockGuard {
    fn drop(&mut self) {
        unlock(&self.lock_file);
    }
}

/// Tries to bootstrap relay availability.
///
/// # Errors
///
/// Returns structured runtime errors for startup timeout, I/O failures, and
/// disabled auto-start.
pub fn bootstrap_relay<F>(
    paths: &RelayRuntimePaths,
    options: BootstrapOptions,
    spawn_relay: F,
) -> Result<BootstrapReport, RuntimeError>
where
    F: FnOnce() -> Result<(), RuntimeError>,
{
    ensure_relay_runtime_directory(paths)?;
    if relay_socket_connectable(paths) {
        return Ok(BootstrapReport {
            spawned_relay: false,
        });
    }
    if !options.auto_start_relay {
        return Err(RuntimeError::RelayAutostartDisabled {
            relay_socket: paths.relay_socket.clone(),
        });
    }

    let mut spawn_relay = Some(spawn_relay);
    match try_acquire_spawn_lock(paths)? {
        Some(_spawn_lock_guard) => {
            if relay_socket_connectable(paths) {
                return Ok(BootstrapReport {
                    spawned_relay: false,
                });
            }
            let _ = remove_stale_relay_socket(paths)?;
            let spawn = spawn_relay.take().expect("spawn closure available");
            spawn()?;
            wait_for_relay_ready(paths, options.startup_timeout)?;
            Ok(BootstrapReport {
                spawned_relay: true,
            })
        }
        None => {
            wait_for_relay_ready(paths, options.startup_timeout)?;
            Ok(BootstrapReport {
                spawned_relay: false,
            })
        }
    }
}

/// Acquires the process lock used by the relay runtime loop.
///
/// # Errors
///
/// Returns an error if another relay already holds the runtime lock.
pub fn acquire_relay_runtime_lock(
    paths: &RelayRuntimePaths,
) -> Result<RelayRuntimeLock, RuntimeError> {
    let mut lock_file = open_lock_file(&paths.relay_lock_file)?;
    let lock_obtained = try_lock_exclusive_nonblocking(&lock_file)?;
    if !lock_obtained {
        return Err(RuntimeError::io(
            "relay already running for state root".to_string(),
            io::Error::new(io::ErrorKind::WouldBlock, "lock held"),
        ));
    }
    write_diagnostic_pid(&mut lock_file)?;
    Ok(RelayRuntimeLock { lock_file })
}

/// Binds the relay socket listener.
///
/// # Errors
///
/// Returns an error when socket bind or permission assignment fails.
pub fn bind_relay_listener(paths: &RelayRuntimePaths) -> Result<UnixListener, RuntimeError> {
    ensure_existing_artifact_is_owned(&paths.relay_socket)?;
    if paths.relay_socket.exists() {
        fs::remove_file(&paths.relay_socket).map_err(|source| {
            RuntimeError::io(
                format!("remove stale relay socket {}", paths.relay_socket.display()),
                source,
            )
        })?;
    }
    let listener = UnixListener::bind(&paths.relay_socket).map_err(|source| {
        RuntimeError::io(
            format!("bind relay socket {}", paths.relay_socket.display()),
            source,
        )
    })?;
    fs::set_permissions(
        &paths.relay_socket,
        fs::Permissions::from_mode(SOCKET_MODE_OWNER_ONLY),
    )
    .map_err(|source| {
        RuntimeError::io(
            format!(
                "set mode 0600 on relay socket {}",
                paths.relay_socket.display()
            ),
            source,
        )
    })?;
    Ok(listener)
}

/// Resolves a best-effort path for the unified `agentmux` executable.
pub fn resolve_relay_program() -> Result<PathBuf, RuntimeError> {
    if let Ok(command_path) = std::env::var("AGENTMUX_COMMAND") {
        let trimmed = command_path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    if let Ok(command_path) = std::env::var("AGENTMUX_RELAY_COMMAND") {
        let trimmed = command_path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }
    }
    let mut sibling = std::env::current_exe()
        .map_err(|source| RuntimeError::io("resolve current executable path", source))?;
    sibling.set_file_name(format!("agentmux{}", std::env::consts::EXE_SUFFIX));
    Ok(sibling)
}

fn relay_socket_connectable(paths: &RelayRuntimePaths) -> bool {
    ensure_existing_artifact_is_owned(&paths.relay_socket).is_ok()
        && UnixStream::connect(&paths.relay_socket).is_ok()
}

// A connectable socket alone is not a sound readiness gate: the kernel queues
// connections into the listen backlog as soon as `bind` returns, well before
// the relay's accept loop starts polling and before SIGINT/SIGTERM handlers
// are installed. The relay publishes `relay.ready` after both are in place
// (see `write_relay_ready_sentinel` in src/commands/host/relay.rs); waiting on
// both signals closes the early-SIGINT window (issues/relay/20) and gives
// callers a real "relay is serving" signal.
fn wait_for_relay_ready(
    paths: &RelayRuntimePaths,
    startup_timeout: Duration,
) -> Result<(), RuntimeError> {
    let deadline = Instant::now() + startup_timeout;
    loop {
        if paths.relay_ready_sentinel.exists() && relay_socket_connectable(paths) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(RuntimeError::RelayStartupTimeout {
                relay_socket: paths.relay_socket.clone(),
                startup_timeout,
            });
        }
        thread::sleep(POLL_SLEEP_INTERVAL);
    }
}

fn try_acquire_spawn_lock(
    paths: &RelayRuntimePaths,
) -> Result<Option<SpawnLockGuard>, RuntimeError> {
    let lock_file = open_lock_file(&paths.relay_spawn_lock_file)?;
    let lock_obtained = try_lock_exclusive_nonblocking(&lock_file)?;
    if lock_obtained {
        return Ok(Some(SpawnLockGuard { lock_file }));
    }
    Ok(None)
}

fn remove_stale_relay_socket(paths: &RelayRuntimePaths) -> Result<bool, RuntimeError> {
    if !paths.relay_socket.exists() {
        return Ok(false);
    }
    if relay_runtime_lock_is_held(paths)? {
        return Ok(false);
    }
    ensure_existing_artifact_is_owned(&paths.relay_socket)?;
    fs::remove_file(&paths.relay_socket).map_err(|source| {
        RuntimeError::io(
            format!("remove stale relay socket {}", paths.relay_socket.display()),
            source,
        )
    })?;
    Ok(true)
}

/// Checks whether the relay runtime lock is currently held.
///
/// # Errors
///
/// Returns `RuntimeError` when lock-file access fails.
pub fn relay_runtime_lock_is_held(paths: &RelayRuntimePaths) -> Result<bool, RuntimeError> {
    let lock_file = open_lock_file(&paths.relay_lock_file)?;
    let lock_obtained = try_lock_exclusive_nonblocking(&lock_file)?;
    if lock_obtained {
        unlock(&lock_file);
        return Ok(false);
    }
    Ok(true)
}

fn open_lock_file(path: &Path) -> Result<std::fs::File, RuntimeError> {
    ensure_existing_artifact_is_owned(path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| {
            RuntimeError::io(
                format!("create lock directory {}", parent.display()),
                source,
            )
        })?;
    }
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|source| RuntimeError::io(format!("open lock file {}", path.display()), source))
}

fn try_lock_exclusive_nonblocking(lock_file: &std::fs::File) -> Result<bool, RuntimeError> {
    let code = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if code == 0 {
        return Ok(true);
    }
    let source = io::Error::last_os_error();
    if source.kind() == io::ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(RuntimeError::io("lock file with flock", source))
}

fn unlock(lock_file: &std::fs::File) {
    let _ = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN) };
}

fn write_diagnostic_pid(lock_file: &mut std::fs::File) -> Result<(), RuntimeError> {
    use std::io::{Seek, SeekFrom, Write};

    lock_file
        .set_len(0)
        .map_err(|source| RuntimeError::io("truncate relay lock file", source))?;
    lock_file
        .seek(SeekFrom::Start(0))
        .map_err(|source| RuntimeError::io("seek relay lock file", source))?;
    writeln!(lock_file, "{}", std::process::id())
        .map_err(|source| RuntimeError::io("write relay lock pid", source))?;
    lock_file
        .flush()
        .map_err(|source| RuntimeError::io("flush relay lock pid", source))
}

#[cfg(test)]
mod tests {
    use std::{
        fs::File,
        io::Write,
        os::unix::net::UnixListener,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    use tempfile::TempDir;

    use super::{BootstrapOptions, bootstrap_relay};
    use crate::runtime::paths::{RelayRuntimePaths, ensure_relay_runtime_directory};

    fn test_paths() -> (TempDir, RelayRuntimePaths) {
        let temporary = TempDir::new().expect("temporary");
        let paths = RelayRuntimePaths::resolve(temporary.path());
        ensure_relay_runtime_directory(&paths).expect("directory");
        (temporary, paths)
    }

    #[test]
    fn bootstrap_fails_when_autostart_disabled() {
        let (_temporary, paths) = test_paths();
        let options = BootstrapOptions {
            auto_start_relay: false,
            startup_timeout: Duration::from_millis(100),
        };
        let err = bootstrap_relay(&paths, options, || Ok(())).expect_err("bootstrap should fail");
        assert!(
            err.to_string()
                .contains("start agentmux host relay with matching --state-directory"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn bootstrap_uses_existing_relay_without_spawning() {
        let (_temporary, paths) = test_paths();
        let _listener = UnixListener::bind(&paths.relay_socket).expect("bind listener");
        let spawn_called = Arc::new(Mutex::new(false));
        let spawn_called_inner = Arc::clone(&spawn_called);
        let options = BootstrapOptions::default();
        let report = bootstrap_relay(&paths, options, || {
            *spawn_called_inner.lock().expect("mutex") = true;
            Ok(())
        })
        .expect("bootstrap");
        assert!(!report.spawned_relay);
        assert!(!*spawn_called.lock().expect("mutex"));
    }

    #[test]
    fn bootstrap_spawns_relay_when_socket_missing() {
        let (_temporary, paths) = test_paths();
        let listener_handle = Arc::new(Mutex::new(None));
        let listener_handle_inner = Arc::clone(&listener_handle);
        let options = BootstrapOptions {
            auto_start_relay: true,
            startup_timeout: Duration::from_secs(1),
        };
        let report = bootstrap_relay(&paths, options, || {
            let relay_socket = paths.relay_socket.clone();
            let ready_sentinel = paths.relay_ready_sentinel.clone();
            let handle = thread::spawn(move || {
                let listener = UnixListener::bind(&relay_socket).expect("bind");
                std::fs::write(&ready_sentinel, b"").expect("write ready sentinel");
                thread::sleep(Duration::from_millis(250));
                drop(listener);
            });
            *listener_handle_inner.lock().expect("mutex") = Some(handle);
            Ok(())
        })
        .expect("bootstrap");
        assert!(report.spawned_relay);
        if let Some(handle) = listener_handle.lock().expect("mutex").take() {
            handle.join().expect("listener thread");
        }
    }

    #[test]
    fn bootstrap_removes_stale_socket_before_spawning() {
        let (_temporary, paths) = test_paths();
        let mut stale = File::create(&paths.relay_socket).expect("stale file");
        writeln!(stale, "stale").expect("write stale");
        let listener_handle = Arc::new(Mutex::new(None));
        let listener_handle_inner = Arc::clone(&listener_handle);

        let options = BootstrapOptions {
            auto_start_relay: true,
            startup_timeout: Duration::from_secs(1),
        };
        let report = bootstrap_relay(&paths, options, || {
            assert!(!paths.relay_socket.exists(), "stale file should be removed");
            let relay_socket = paths.relay_socket.clone();
            let ready_sentinel = paths.relay_ready_sentinel.clone();
            let handle = thread::spawn(move || {
                let listener = UnixListener::bind(&relay_socket).expect("bind");
                std::fs::write(&ready_sentinel, b"").expect("write ready sentinel");
                thread::sleep(Duration::from_millis(250));
                drop(listener);
            });
            *listener_handle_inner.lock().expect("mutex") = Some(handle);
            Ok(())
        })
        .expect("bootstrap");
        assert!(report.spawned_relay);
        if let Some(handle) = listener_handle.lock().expect("mutex").take() {
            handle.join().expect("listener thread");
        }
    }
}
