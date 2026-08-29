use std::{ops::Deref, path::Path};
use tempfile::TempDir;

pub struct GuardedTempDir {
    inner: TempDir,
}

impl GuardedTempDir {
    pub fn new() -> Self {
        Self {
            inner: TempDir::new().expect("temporary"),
        }
    }

    pub fn path(&self) -> &Path {
        self.inner.path()
    }
}

impl Deref for GuardedTempDir {
    type Target = TempDir;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl AsRef<Path> for GuardedTempDir {
    fn as_ref(&self) -> &Path {
        self.inner.path()
    }
}

impl Drop for GuardedTempDir {
    fn drop(&mut self) {
        agentmux::relay::test_cleanup_acp_workers(self.inner.path());
        // Kill acp_stub children logged to acp_child_pids.txt, gated on
        // /proc/<pid>/cmdline containing acp_stub.sh to avoid signalling a
        // recycled pid that no longer belongs to us. This belongs in the
        // harness (which owns acp_child_pid_path) not in src/relay, which
        // should not parse a test fixture's file format.
        let pid_file = self.inner.path().join("acp_child_pids.txt");
        if let Ok(contents) = std::fs::read_to_string(&pid_file) {
            for line in contents.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(pid) = parts.get(1).and_then(|s| s.parse::<i32>().ok()) {
                    let cmdline_path = format!("/proc/{pid}/cmdline");
                    if let Ok(cmdline) = std::fs::read_to_string(&cmdline_path) {
                        if !cmdline.contains("acp_stub.sh") {
                            continue;
                        }
                    } else {
                        continue;
                    }
                    unsafe {
                        ::libc::kill(pid, ::libc::SIGTERM);
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        if let Ok(contents) = std::fs::read_to_string(&pid_file) {
            for line in contents.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if let Some(pid) = parts.get(1).and_then(|s| s.parse::<i32>().ok()) {
                    let cmdline_path = format!("/proc/{pid}/cmdline");
                    if let Ok(cmdline) = std::fs::read_to_string(&cmdline_path) {
                        if !cmdline.contains("acp_stub.sh") {
                            continue;
                        }
                    } else {
                        continue;
                    }
                    unsafe {
                        ::libc::kill(pid, ::libc::SIGKILL);
                    }
                }
            }
        }
    }
}
