//! Shared authorization domain types: the resolved [`AuthorizationContext`] a
//! request is checked against, plus the policy primitives ([`PolicyControls`],
//! [`PolicyScope`], [`UiSessionAuthorization`]) the three behavioral seams
//! (`loading`, `resolution`, `checks`) operate on. Members are `pub(super)` so
//! the sibling seams can construct and read them; nothing here makes a decision.

use std::collections::HashMap;

#[derive(Clone, Debug)]
pub(in crate::relay) struct AuthorizationContext {
    pub(super) controls_by_session: HashMap<String, PolicyControls>,
    pub(super) ui_sessions: HashMap<String, UiSessionAuthorization>,
    pub(super) choices_pending_max: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PolicyScope {
    None,
    SelfOnly,
    Home,
    All,
}

impl PolicyScope {
    fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::SelfOnly => 1,
            Self::Home => 2,
            Self::All => 3,
        }
    }

    pub(super) fn allows(self, minimum: Self) -> bool {
        self.rank() >= minimum.rank()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PolicyControls {
    pub(super) find: PolicyScope,
    pub(super) list: PolicyScope,
    pub(super) look: PolicyScope,
    pub(super) send: PolicyScope,
    pub(super) raww: PolicyScope,
    pub(super) choose: PolicyScope,
    pub(super) updown: PolicyScope,
    pub(super) do_controls: HashMap<String, PolicyScope>,
    pub(super) new_controls: HashMap<String, PolicyScope>,
    pub(super) change_controls: HashMap<String, PolicyScope>,
}

#[derive(Clone, Debug)]
pub(super) struct UiSessionAuthorization {
    pub(super) display_name: Option<String>,
}

impl PolicyControls {
    pub(super) fn conservative_default() -> Self {
        Self {
            find: PolicyScope::SelfOnly,
            list: PolicyScope::Home,
            look: PolicyScope::Home,
            send: PolicyScope::Home,
            // raww is raw keystroke-grade input injection; the in-code fallback
            // (no `default` preset and no member `policy_id`) must close it by
            // default too, matching choose/updown and the serde-omission default.
            raww: PolicyScope::None,
            choose: PolicyScope::None,
            updown: PolicyScope::None,
            do_controls: HashMap::new(),
            new_controls: HashMap::new(),
            change_controls: HashMap::new(),
        }
    }
}
