//! List-view formatting for the TUI recipient picker.
//!
//! Captures bundle-scoped and session-scoped fields surfaced by `relay::List`
//! so the picker can show:
//!
//! - whether the bundle is hosted and what its lifecycle state is — see
//!   [`BundleStatusDisplay`]. Distinguishing `hosted=true, state=down`
//!   (sessions failed startup) from `hosted=false, state=down` (never started
//!   or shut down) is the load-bearing reason that summary exists.
//! - whether each individual session is ready — see
//!   [`format_recipient_picker_label`] / [`RecipientReadiness`]. The bundle
//!   aggregate (`startup_health`) can be `Degraded` while individual sessions
//!   are still `ready=false`, and the operator needs the per-session detail.

use crate::relay::{
    ListedBundle, ListedBundleStartupHealth, ListedBundleState, StartupFailureRecord,
};

/// Snapshot of bundle-level status fields used by the TUI list view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleStatusDisplay {
    pub namespace: String,
    pub hosted: bool,
    pub state: ListedBundleState,
    pub startup_health: Option<ListedBundleStartupHealth>,
    pub state_reason_code: Option<String>,
    pub startup_failure_count: usize,
    pub startup_failures: Vec<StartupFailureSummary>,
}

/// One per-session startup failure surfaced beneath the bundle status header.
/// Carries only the render-relevant fields (`session_id`, `code`, `reason`),
/// dropping the wire record's `timestamp`, `sequence`, and `details` — the last
/// of which (a `serde_json::Value`) is only `PartialEq`, so keeping it would
/// block [`BundleStatusDisplay`] from deriving `Eq`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupFailureSummary {
    pub session_id: String,
    pub code: String,
    pub reason: String,
}

impl BundleStatusDisplay {
    pub fn from_listed_bundle(bundle: &ListedBundle) -> Self {
        Self {
            namespace: bundle.id.clone(),
            hosted: bundle.hosted,
            state: bundle.state.clone(),
            startup_health: bundle.startup_health.clone(),
            state_reason_code: bundle.state_reason_code.clone(),
            startup_failure_count: bundle.startup_failure_count,
            startup_failures: bundle
                .recent_startup_failures
                .iter()
                .map(StartupFailureSummary::from_record)
                .collect(),
        }
    }
}

impl StartupFailureSummary {
    fn from_record(record: &StartupFailureRecord) -> Self {
        Self {
            session_id: record.session_id.clone(),
            code: record.code.clone(),
            reason: record.reason.clone(),
        }
    }
}

/// Format the bundle status into a single k=v line styled like CLI list output.
///
/// Always includes `bundle`, `hosted`, and `state`. Includes `startup_health`
/// and `reason_code` only when present. `startup_failure_count` is included
/// when non-zero to keep `hosted=true, state=down` failures visible.
pub fn format_bundle_status_line(status: &BundleStatusDisplay) -> String {
    let hosted_token = if status.hosted { "yes" } else { "no" };
    let state_token = listed_bundle_state_token(&status.state);
    let mut parts = vec![
        format!("bundle={}", status.namespace),
        format!("hosted={hosted_token}"),
        format!("state={state_token}"),
    ];
    if let Some(health) = status.startup_health.as_ref() {
        parts.push(format!(
            "startup_health={}",
            listed_bundle_startup_health_token(health)
        ));
    }
    if let Some(reason_code) = status.state_reason_code.as_ref() {
        parts.push(format!("reason_code={reason_code}"));
    }
    if status.startup_failure_count > 0 {
        parts.push(format!(
            "startup_failure_count={}",
            status.startup_failure_count
        ));
    }
    parts.join(" ")
}

/// One k=v detail line per recent per-session startup failure, ordered oldest
/// to newest as `load_startup_failures` returns them. Rendered beneath the
/// bundle status header so the operator sees why individual sessions failed,
/// not just the aggregate count. `reason` is placed last because it is free
/// text that may contain spaces.
pub fn format_startup_failure_lines(status: &BundleStatusDisplay) -> Vec<String> {
    status
        .startup_failures
        .iter()
        .map(|failure| {
            format!(
                "startup_failure session={} code={} reason={}",
                failure.session_id, failure.code, failure.reason
            )
        })
        .collect()
}

/// Severity classification for the bundle status, used to choose a render
/// style. Three buckets distinguish the visually load-bearing states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BundleStatusSeverity {
    /// `hosted=true, state=up, startup_health=healthy` — fully operational.
    Healthy,
    /// `hosted=true, state=up, startup_health=degraded|None` or any other
    /// degraded-but-still-hosted condition.
    Degraded,
    /// `hosted=true, state=down` — bundle is hosted but every session failed
    /// to start. Distinct from [`BundleStatusSeverity::Unhosted`] because the
    /// relay still owns the bundle and operator action is required.
    HostedDown,
    /// `hosted=false, state=down` — bundle has never been started or has been
    /// shut down. Operator can bring it up.
    Unhosted,
}

pub fn bundle_status_severity(status: &BundleStatusDisplay) -> BundleStatusSeverity {
    match (status.hosted, &status.state) {
        (true, ListedBundleState::Up) => match status.startup_health {
            Some(ListedBundleStartupHealth::Healthy) => BundleStatusSeverity::Healthy,
            _ => BundleStatusSeverity::Degraded,
        },
        (true, ListedBundleState::Down) => BundleStatusSeverity::HostedDown,
        (false, _) => BundleStatusSeverity::Unhosted,
    }
}

/// Per-session readiness classification for picker rendering.
///
/// Mirrors `relay::ListedSession::ready`. Kept as a pure enum (not a
/// ratatui `Style`) so render styling can be co-located with the rest of the
/// rendering code while pure formatting/classification stays testable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RecipientReadiness {
    Ready,
    NotReady,
}

impl RecipientReadiness {
    pub fn from_ready(ready: bool) -> Self {
        if ready { Self::Ready } else { Self::NotReady }
    }
}

/// Format one recipient row for the picker list.
///
/// `ready=true` rows render as the canonical `id (display_name)` (or just `id`
/// when no display name is known). `ready=false` rows append a `[not ready]`
/// marker so the state is legible even without color (color-blind operators,
/// terminals that drop styling).
pub fn format_recipient_picker_label(
    session_name: &str,
    display_name: Option<&str>,
    readiness: RecipientReadiness,
) -> String {
    let base = match display_name {
        Some(name) => format!("{session_name} ({name})"),
        None => session_name.to_string(),
    };
    match readiness {
        RecipientReadiness::Ready => base,
        RecipientReadiness::NotReady => format!("{base}  [not ready]"),
    }
}

fn listed_bundle_state_token(state: &ListedBundleState) -> &'static str {
    match state {
        ListedBundleState::Up => "up",
        ListedBundleState::Down => "down",
    }
}

fn listed_bundle_startup_health_token(health: &ListedBundleStartupHealth) -> &'static str {
    match health {
        ListedBundleStartupHealth::Healthy => "healthy",
        ListedBundleStartupHealth::Degraded => "degraded",
    }
}
