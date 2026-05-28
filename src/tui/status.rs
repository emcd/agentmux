//! Bundle status formatting for the TUI list view.
//!
//! Captures the bundle-scoped fields surfaced by `relay::ListedBundle` so the
//! recipient picker can show whether the bundle is hosted and what its
//! lifecycle state is. Distinguishing `hosted=true, state=down` (sessions
//! failed startup) from `hosted=false, state=down` (never started or shut
//! down) is the load-bearing reason this exists.

use crate::relay::{ListedBundle, ListedBundleStartupHealth, ListedBundleState};

/// Snapshot of bundle-level status fields used by the TUI list view.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundleStatusDisplay {
    pub bundle_name: String,
    pub hosted: bool,
    pub state: ListedBundleState,
    pub startup_health: Option<ListedBundleStartupHealth>,
    pub state_reason_code: Option<String>,
    pub startup_failure_count: usize,
}

impl BundleStatusDisplay {
    pub fn from_listed_bundle(bundle: &ListedBundle) -> Self {
        Self {
            bundle_name: bundle.id.clone(),
            hosted: bundle.hosted,
            state: bundle.state.clone(),
            startup_health: bundle.startup_health.clone(),
            state_reason_code: bundle.state_reason_code.clone(),
            startup_failure_count: bundle.startup_failure_count,
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
        format!("bundle={}", status.bundle_name),
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
