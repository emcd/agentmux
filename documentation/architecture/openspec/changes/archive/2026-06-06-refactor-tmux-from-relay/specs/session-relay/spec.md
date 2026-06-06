## ADDED Requirements

### Requirement: Extracted Tmux Module Boundary

The relay SHALL expose tmux operations through a dedicated `tmux` module with a public interface. The tmux module SHALL provide pane targeting, snapshot capture, text injection, cursor resolution, activity marker resolution, operator interaction detection, and session lifecycle operations as module-level free functions. All relay handlers and delivery code SHALL consume tmux operations through this module interface rather than via internal `pub(super)` visibility.

#### Scenario: Relay consumes tmux through public module interface

- **WHEN** relay handlers or delivery code require tmux operations
- **THEN** they access them through the `crate::tmux` module public interface
- **AND** no `pub(super)` visibility is used for tmux operations

#### Scenario: Tmux session lifecycle owned by tmux module

- **WHEN** relay needs to create, prune, or reconcile tmux sessions
- **THEN** it uses session lifecycle operations provided by the tmux module

#### Scenario: Pane targeting and injection through tmux module

- **WHEN** relay look, delivery, or quiescence operations need pane targeting, snapshot capture, or text injection
- **THEN** they use pane operations provided by the tmux module

#### Scenario: Tmux socket path passed explicitly

- **WHEN** tmux operations are invoked
- **THEN** the tmux socket path is passed as an explicit argument (not stored in module state)