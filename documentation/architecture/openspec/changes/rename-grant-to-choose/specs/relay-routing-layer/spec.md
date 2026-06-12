## MODIFIED Requirements

### Requirement: Operation Body Contract

Operation handler bodies SHALL receive a `ResolvedRoute` whose targets are
already classified (located to a bundle or the relay-wide registry) and
authorized. Handler bodies SHALL NOT:

- Parse `@<namespace>` suffixes from principal IDs.
- Evaluate requester policy controls or classify target scope tiers.

Handler bodies MAY load the target bundles' configuration to validate target
existence and to assemble delivery (member transport, choice deciders,
runtime directory) — this is delivery work, distinct from routing and
authorization. They SHALL implement only operation-specific work: existence
validation, snapshot capture, delivery enqueueing, raw text injection, session
enumeration, or lifecycle control.

#### Scenario: Handler body free of routing and authorization logic

- **WHEN** a developer reads any target-operation handler (`handle_send`,
  `handle_look`, `handle_raww`, `handle_list`)
- **THEN** no principal-ID suffix parsing and no requester-policy or scope-tier
  evaluation are present
- **AND** routing classification and authorization are handled exclusively by
  the dispatch layer, with only existence validation and delivery assembly
  remaining in the body
