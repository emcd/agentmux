# Tasks

## 1. Watcher inscriptions

- [x] 1.1 Carry relay error `code` and structured `details` through
      `relay.bundle.load_failed` inscription payloads

## 2. Host startup failure reporting

- [x] 2.1 Preserve relay-layer error details into the startup summary's
      per-bundle entries (`details` field)
- [x] 2.2 Emit a per-bundle stderr reason and a `relay.bundle.startup_failed`
      inscription for every failed bundle before the host exits
- [x] 2.3 Emit the startup summary inscription on the fatal startup path

## 3. Error message clarity

- [x] 3.1 Reword policy scope rejections to "unknown scope value" and list
      the expected scope ladder in the error details

## 4. Specs and tests

- [x] 4.1 MODIFIED cli-surface "Relay Host Startup Summary Contract": add
      nullable `details` to per-bundle entries
- [x] 4.2 Integration test: all-bundles-failed startup exits nonzero with
      per-bundle stderr reason and `relay.bundle.startup_failed` inscription
      carrying the offending policy control and value
