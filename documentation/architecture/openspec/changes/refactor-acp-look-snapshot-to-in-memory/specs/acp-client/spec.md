## ADDED Requirements

### Requirement: Non-Draining Replay Buffer Accessor

The shared ACP client module SHALL expose a non-draining accessor that
returns a snapshot of the live replay buffer entries without consuming
or removing them. This accessor serves the relay look path, which
requires repeated reads of the same buffer state without disturbing
other consumers.

The existing draining accessor (which returns and removes entries)
SHALL be retained for non-look consumers such as the debug TUI binary
that intentionally drain entries on each iteration.

#### Scenario: Non-draining accessor returns current entries without consumption

- **WHEN** the relay look path calls the non-draining replay accessor
- **THEN** the call returns a snapshot of all currently-buffered replay entries in receive order
- **AND** the underlying buffer state is unchanged after the call

#### Scenario: Draining accessor remains available for debug TUI

- **WHEN** the debug TUI binary calls the draining replay accessor
- **THEN** the call returns and removes the buffered entries as it does today
- **AND** the non-draining accessor continues to function for relay look reads on other ACP clients

### Requirement: Replay Buffer Cap and Eviction

The shared ACP client module SHALL enforce a maximum entry count on the
live replay buffer with oldest-evict-first semantics. The cap SHALL be
1000 entries, mirroring the prior persisted-path retention bound.

When ingesting a new replay entry would exceed the cap, the oldest
entry SHALL be evicted to maintain the bound.

#### Scenario: Evict oldest entry when buffer reaches cap

- **WHEN** the live replay buffer holds 1000 entries
- **AND** a new replay entry is ingested
- **THEN** the oldest entry is evicted
- **AND** the buffer continues to hold exactly 1000 entries
- **AND** the most recent entry is the newly ingested one
