//! What a spawned member actually receives from the relay that started it.
//!
//! Two levels of evidence, deliberately both. Most tests here drive
//! `agentmux up` against a fake tmux and assert on the argument vector tmux was
//! handed, which pins the stamp precisely and runs in milliseconds.
//! `a_relay_spawned_member_client_reaches_the_spawning_relay` instead uses real
//! tmux and a real `agentmux host mcp` descendant, because argv shows a value
//! being passed while the defect being guarded against is a child *resolving*
//! somewhere the relay never bound. Only a live child arriving at the right
//! socket rules that out.
//!
//! The cluster files partition the 10 tests by concern:
//! - [`rendezvous`]: rendezvous end-to-end through a real spawned member and
//!   the partial-line wait helper (`a_partially_written_response_line_is_not_counted_as_complete`,
//!   `a_relay_spawned_member_client_reaches_the_spawning_relay`,
//!   `a_blank_declaration_still_leaves_the_member_able_to_reach_the_relay`).
//! - [`spawn_stamp`]: argv inspection of the stamp under declared-state-root
//!   and blank-state-root fixtures
//!   (`a_blank_member_declaration_does_not_suppress_the_stamp`,
//!   `a_spawned_member_receives_the_relays_state_root_over_its_own_declaration`).
//! - [`tmux_socket`]: deep state-root sun_path overshoot and tmux-command
//!   resolution against the launch directory and a bare-name PATH
//!   (`a_relay_comes_up_under_a_state_root_longer_than_sun_path`,
//!   `a_relative_tmux_wrapper_still_resolves_against_the_launch_directory`,
//!   `a_bare_tmux_command_resolves_through_the_launch_directorys_path`,
//!   `a_bare_tmux_command_keeps_execvp_search_order_across_relative_entries`,
//!   `tmux_is_addressed_relative_to_its_own_socket_directory`).
//!
//! Shared helpers (`declare_member_state_directory`, `recorded_new_session`,
//! `MEMBER_DECLARED_STATE_ROOT`, `MEMBER_BLANK_STATE_ROOT`) live in
//! [`helpers`].

mod helpers;
mod rendezvous;
mod spawn_stamp;
mod tmux_socket;
