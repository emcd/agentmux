# credential-path-confinement Specification

## Purpose

Credential writes beneath a relay-owned state root remain confined to trusted,
owner-controlled directory handles.

## Requirements

### Requirement: Symlink-Safe Credential Sink Traversal

The system SHALL traverse path components without following symlinks for every
state-root-owned credential write: the sink's directory creation, permission
changes, temporary-file creation, and rename publication for both `new peer`
and `change psk`, the peer-slot install operation's directory creation, file
opens, temporary-file creation, rename publication, and cleanup for
`peers/<alias>.psk`, plus principal-store load and persist. Each ancestor
component SHALL resolve to a real directory or be created by the relay with
owner-only permissions, and a symlinked ancestor component SHALL abort the
operation with `validation_invalid_credential_path` naming the offending
component. The final-path symlink refusal and owner-only file modes of the
existing sink remain in force. `Path` (caller-named, caller-owned)
destinations are excluded from ancestor traversal and keep the final-target
symlink check only, because their pre-existing parents are outside the state
root trust anchor and routinely contain legitimate symlinks.

#### Scenario: Ancestor symlink rejected for new peer provisioning

- **WHEN** a `new peer` credential write traverses an ancestor directory that
  is a symlink
- **THEN** the relay aborts with `validation_invalid_credential_path` naming
  the symlinked component
- **AND** does not register the principal

#### Scenario: Ancestor symlink rejected for PSK rotation

- **WHEN** a `change psk` credential write traverses an ancestor directory
  that is a symlink
- **THEN** the relay aborts with `validation_invalid_credential_path` naming
  the symlinked component
- **AND** does not rotate the PSK or revoke any connection

#### Scenario: Symlinked store directory is neither read nor changed

- **WHEN** principal-store load or persist traverses a symlinked `identity/`
  ancestor
- **THEN** the operation aborts with `validation_invalid_credential_path`
  naming the symlinked component
- **AND** the external symlink target is neither read nor modified

#### Scenario: Relay-created parents are owner-only and symlink-free

- **WHEN** a credential write creates missing relay-owned ancestor directories
- **THEN** each is created with mode 0700 and contains no symlinked component
- **AND** the credential file lands with mode 0600

#### Scenario: Post-rename directory-sync failure preserves the registration

- **WHEN** a `new peer` credential write fails directory sync after rename
  publication
- **THEN** the relay reports a typed durability-uncertain outcome
- **AND** the committed record stands instead of rolling back, leaving the
  published file credentialed

#### Scenario: Post-rename directory-sync failure preserves the rotation

- **WHEN** a `change psk` credential write fails directory sync after rename
  publication
- **THEN** the relay reports a typed durability-uncertain outcome
- **AND** the rotated record stands with the superseded credential revoked

### Requirement: Retained Directory Handle Publication

The system SHALL execute directory creation, file opens, temporary-file
creation, rename publication, and cleanup relative to retained directory
handles anchored at the trusted state root after staging, never by re-walking
pathnames. Publication therefore cannot be redirected outside the state tree by
any exchange: an ancestor exchange persisting at commit SHALL abort the
operation with `validation_invalid_credential_path` rather than publish, while
handle-relative publication through an already verified chain stays inside the
tree by construction.

#### Scenario: Ancestor exchange after staging aborts before publish

- **WHEN** a credential write stages its destination and an ancestor component
  is then exchanged for a symlink before commit
- **THEN** the relay aborts with `validation_invalid_credential_path`
- **AND** publishes nothing through the exchanged component
