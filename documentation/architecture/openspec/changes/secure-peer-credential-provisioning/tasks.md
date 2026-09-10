## 1. Confinement helper

- [x] 1.1 Add a shared no-follow traversal helper for state-root-owned credential writes (per-component walk, `validation_invalid_credential_path` naming the component, owner-only modes on created directories, retained dirfds anchored at the state root for all post-staging operations).
- [x] 1.2 Route the credential sink's directory creation, permission changes, temp creation, rename, and cleanup through the helper for both `new peer` and `change psk`, plus the peer-slot install's directory creation, file opens, temp creation, rename, and cleanup.
- [x] 1.3 Route `PrincipalStore` load and persist through the helper (mandatory). Verification: existing concurrency suite passes unchanged, fault-injection paths (`.fault-pre-rename`, `.fault-dir-sync`) exercise the same error paths, on-disk shape after `persist` is byte-identical to today.
- [x] 1.4 Keep `Path` destinations on final-target symlink check only (no ancestor traversal, no creation).

## 2. Peer-link protocol

- [x] 2.1 Implement the relay-side install operation (alias-referent binding, absent/identical/different authorization classification under a per-slot lock, 0700/0600 permissions, idempotent same-content rewrite, temp-write plus file fsync plus atomic rename plus directory fsync with typed post-rename uncertainty, PSK omitted from every install response).
- [x] 2.2 Implement CLI `link peer` one-way mode (register, issue, install; explicit registration-mint discard; second-claim tolerance on registration; halt on issuance failure).
- [x] 2.3 Implement CLI `link peer` paired mode (declared cross-equality verification before either mint, two alias registrations retaining both mints, cross-installs, zero drops) plus the one-way-to-bidirectional upgrade path (rotate discarded alias credential, install on the peer).
- [x] 2.4 Implement endpoint selection (two state roots, `<state-root>/relay.sock` derivation, both relays live; entries added after linking) and mode-specific timeout recovery (one-way registration disambiguation, paired rotate-to-known, issuance rotate-or-drop, install same-PSK retry).
- [x] 2.5 Implement MCP `link` meta-tool with `command="peer"` mirroring the install contract (explicit secret argument, no logging or persistence of the secret).

## 3. Tests

- [x] 3.1 Add ancestor-symlink rejection tests for `new peer` and `change psk` sink operations asserting `validation_invalid_credential_path` (unit).
- [x] 3.2 Add store load/persist symlink tests proving the external target is neither read nor changed (unit).
- [x] 3.3 Add ancestor-exchange-after-staging tests asserting abort before publish, covering sink and install writes (unit).
- [x] 3.4 Add install tests: referent binding, three-way authorization classification, idempotent rewrite, unsafe-component refusal (unit + integration, both surfaces).
- [x] 3.5 Add the pinned retry test: successful write, lost response, same-authority (`new.peer=all` only) identical retry succeeds (integration).
- [x] 3.6 Add link-flow tests: one-way bootstrap order, second-claim tolerance (one-way only), halt on issuance failure, paired zero-drop install, cross-equality mismatch abort before any mint, upgrade path, mode-specific timeout recoveries (one-way retry-proceed vs paired rotate-to-known distinguishing test), post-rename durability uncertainty (integration, both surfaces).
- [x] 3.7 Add transfer-boundary tests: issuance Response carries exactly one PSK, all other responses omit it, no secret in logs/snippets/diagnostics/outputs (integration).

## 4. Verification and review

- [x] 4.1 Run the full test suite, clippy, fmt, and `openspec validate --all --strict`.
- [ ] 4.2 Confirm `new peer` / `change psk` Config semantics remain session-only (no widening).
- [ ] 4.3 Confirm `agentmux:issues/relay/83` remains untouched (separate documentation work).
