# Source Notes

## Starter Template Embedding

Starter configuration templates are version-controlled and embedded into the
binary with `include_str!`:

- coders template: `data/configuration/coders.toml`
- bundle template: `data/configuration/bundle.toml`

Runtime startup copies these templates into configuration roots only when the
target files are missing.
