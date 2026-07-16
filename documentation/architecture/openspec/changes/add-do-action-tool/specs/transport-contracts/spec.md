## ADDED Requirements

### Requirement: Relay Do Operation

Relay SHALL expose a `do` operation with modes:

- `list`
- `show`
- `run`

`list` returns available action ids and optional descriptions.
`show` returns metadata for one action.
`run` dispatches configured prompt injection for one action.

Run mode request contract SHALL include:

- required `mode=run`
- required `action`
- no target selector fields in alpha scope

#### Scenario: Return available actions for list mode

- **WHEN** relay receives `do` request with `mode=list`
- **THEN** relay returns action catalog for sender/session context

#### Scenario: Return action metadata for show mode

- **WHEN** relay receives `do` request with `mode=show` for configured
  action id
- **THEN** relay returns metadata for that action

#### Scenario: Reject unknown action on run mode

- **WHEN** relay receives `do` run request for unknown action id
- **THEN** relay returns `validation_unknown_action`

#### Scenario: Reject target selector fields for do run

- **WHEN** relay receives do run request with `target_session` or
  `target_sessions`
- **THEN** relay returns `validation_invalid_arguments`


### Requirement: Relay Do Run Acceptance Payload

Successful `do` `run` response SHALL include required fields:

- `schema_version`
- `bundle_name`
- `requester_session`
- `action`
- `status` (`accepted`)
- `outcome` (`queued`)
- `message_id`

#### Scenario: Return canonical acceptance payload for do run

- **WHEN** relay accepts `do` run request for configured action
- **THEN** relay response includes all required acceptance payload fields
