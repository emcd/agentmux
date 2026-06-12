# Tasks

## 1. Recipient list

- [x] 1.1 Build the cross-group recipient list as canonical ids in
      `execute_send` and carry it on every delivery task
- [x] 1.2 Canonicalize the single-entry recipient list on the raww path

## 2. Envelope rendering

- [x] 2.1 Render From/To/Cc identity tokens as canonical
      `session@namespace` ids
- [x] 2.2 Derive Cc entries from the full recipient list, with display names
      for delivery-bundle members and canonical-id fallback for
      cross-namespace co-recipients
- [x] 2.3 Pass canonical co-recipient ids through UI stream
      `incoming_message` events without re-qualification

## 3. Specs and tests

- [x] 3.1 MODIFIED pane-envelope "Address Identity Format" and "CC
      Informational Semantics"
- [x] 3.2 Integration test: cross-bundle multi-target send delivers envelopes
      with canonical From/To addresses and cross-namespace co-recipients in Cc
