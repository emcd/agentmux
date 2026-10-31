//! `adapter.persistState` writes.
//!
//! Tag-echoed writes under the current generation (never checking
//! currency itself) with client-side duplicate-id tracking and an
//! explicit adapter-local finite cap on outstanding entries. The
//! frozen hello carries no persist-specific in-flight cap, so none is
//! assumed: the bound here is adapter-local.

use std::collections::HashMap;

/// Refusal of an originated persist write before it reaches the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PersistRefusal {
    DuplicateWriteId { write_id: String },
    OverLocalCap { cap: usize },
}

/// Resume-store tag echoed on a persist write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeTag {
    pub runtime_instance: String,
    pub target_session: String,
    pub adapter_generation: u64,
}
/// Tracks originated persist writes by write id for duplicate
/// detection and cap enforcement.
#[derive(Debug)]
pub struct PersistTracker {
    cap: usize,
    outstanding: HashMap<String, ResumeTag>,
}

impl PersistTracker {
    /// Creates a tracker allowing `cap` outstanding writes.
    #[must_use]
    pub fn new(cap: usize) -> Self {
        Self {
            cap,
            outstanding: HashMap::new(),
        }
    }

    /// Registers a write, refusing duplicates and cap overruns.
    ///
    /// # Errors
    ///
    /// Returns [`PersistRefusal::DuplicateWriteId`] for a repeated id
    /// and [`PersistRefusal::OverLocalCap`] past the cap.
    pub fn submit(
        &mut self,
        write_id: String,
        tag: ResumeTag,
    ) -> Result<(), PersistRefusal> {
        if self.outstanding.contains_key(&write_id) {
            return Err(PersistRefusal::DuplicateWriteId { write_id });
        }
        if self.outstanding.len() >= self.cap {
            return Err(PersistRefusal::OverLocalCap { cap: self.cap });
        }
        self.outstanding.insert(write_id, tag);
        Ok(())
    }

    /// Retires a write on its commit or refusal outcome.
    pub fn retire(&mut self, write_id: &str) {
        self.outstanding.remove(write_id);
    }

    /// Counts outstanding writes.
    #[must_use]
    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }
}
