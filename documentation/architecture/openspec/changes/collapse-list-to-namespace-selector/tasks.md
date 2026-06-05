## 1. MCP surface

- [x] 1.1 Replace `ListArgs { all, bundle_name }` with
  `namespace: Option<String>` in `src/mcp/params.rs`; rename the `list`
  command selector constant `sessions` → `principals`
- [x] 1.2 Rewrite list dispatch in `src/mcp/server.rs`: resolve `namespace`
  (null→home, `<bundle>`, `GLOBAL`, `*`→fan-out); drop the `all`/`bundle_name`
  mutual-exclusion path; keep adapter-owned `*` fan-out (lexicographic, fail
  fast on first `authorization_forbidden`)
- [x] 1.3 Rename the response collection key `sessions` → `principals` in the
  list response map
- [x] 1.4 Update `src/mcp/help.rs` + fixtures: `list.sessions` → `list.principals`,
  `command="principals"`, new `namespace` arg schema

## 2. Relay contract

- [x] 2.1 Rename `ListedBundle.sessions` → `principals` in
  `src/relay/contract.rs` (serde key follows the field); update construction
  sites in `src/relay/handlers.rs`

## 3. CLI surface

- [x] 3.1 Replace `agentmux list sessions --bundle/--all` with
  `agentmux list principals --namespace <ns>` (`*` for fan-out); drop the
  flag mutual-exclusion
- [x] 3.2 Rename the CLI machine-output collection key `sessions[]` →
  `principals[]`

## 4. Consumers

- [x] 4.1 Update any `src/tui/**` (and other) consumers of the `sessions`
  collection key to `principals`

## 5. Tests + docs

- [x] 5.1 Update MCP/CLI list tests for the `namespace` selector, `*` fan-out,
  `GLOBAL`, and the `principals` key
- [x] 5.2 Refresh `src/mcp/README.md` list-tool documentation
- [x] 5.3 `openspec validate collapse-list-to-namespace-selector --strict`;
  fmt + clippy + unit/integration suites green
