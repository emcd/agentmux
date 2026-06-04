## 1. MCP surface

- [ ] 1.1 Replace `ListArgs { all, bundle_name }` with
  `namespace: Option<String>` in `src/mcp/params.rs`; rename the `list`
  command selector constant `sessions` → `principals`
- [ ] 1.2 Rewrite list dispatch in `src/mcp/server.rs`: resolve `namespace`
  (null→home, `<bundle>`, `GLOBAL`, `*`→fan-out); drop the `all`/`bundle_name`
  mutual-exclusion path; keep adapter-owned `*` fan-out (lexicographic, fail
  fast on first `authorization_forbidden`)
- [ ] 1.3 Rename the response collection key `sessions` → `principals` in the
  list response map
- [ ] 1.4 Update `src/mcp/help.rs` + fixtures: `list.sessions` → `list.principals`,
  `command="principals"`, new `namespace` arg schema

## 2. Relay contract

- [ ] 2.1 Rename `ListedBundle.sessions` → `principals` in
  `src/relay/contract.rs` (serde key follows the field); update construction
  sites in `src/relay/handlers.rs`

## 3. CLI surface

- [ ] 3.1 Replace `agentmux list sessions --bundle/--all` with
  `agentmux list principals --namespace <ns>` (`*` for fan-out); drop the
  flag mutual-exclusion
- [ ] 3.2 Rename the CLI machine-output collection key `sessions[]` →
  `principals[]`

## 4. Consumers

- [ ] 4.1 Update any `src/tui/**` (and other) consumers of the `sessions`
  collection key to `principals`

## 5. Tests + docs

- [ ] 5.1 Update MCP/CLI list tests for the `namespace` selector, `*` fan-out,
  `GLOBAL`, and the `principals` key
- [ ] 5.2 Refresh `src/mcp/README.md` list-tool documentation
- [ ] 5.3 `openspec validate collapse-list-to-namespace-selector --strict`;
  fmt + clippy + unit/integration suites green
