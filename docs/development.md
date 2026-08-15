# Development

[Back to the project README](../README.md)

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/token-toxication-server` | Axum server, routing policy, SQLite persistence, OpenAPI, and embedded admin assets |
| `apps/admin` | React, TypeScript, Vite+, and shadcn admin interface |
| `Justfile` | Local development, generation, and CI commands |
| `.github/workflows/ci.yml` | Rust and admin CI jobs |

## Prerequisites

- Rust 1.95 or newer
- [just](https://github.com/casey/just)
- Vite+ with the `vp` command available
- `openapi-nexus` 0.1.17 for SDK generation

## Common commands

| Command | Purpose |
| --- | --- |
| `just dev-server` | Run the backend and serve the built admin interface |
| `just ui-dev` | Run the admin development server |
| `just fmt` | Format the Rust workspace |
| `just clippy` | Run Clippy with warnings denied |
| `just test` | Run the Rust test suite |
| `just openapi-generate` | Generate the OpenAPI document |
| `just sdk-generate` | Generate and format the TypeScript admin SDK |
| `just ui-check` | Run frontend formatting, lint, and type checks |
| `just ui-build` | Build the admin interface |
| `just ci` | Run the full local CI pipeline |

Run `just sdk-generate` after changing OpenAPI schemas or routes. The generated OpenAPI document is written to `openapi/token-toxication.openapi.json`, and the generated TypeScript SDK is written to `apps/admin/src/generated/token-toxication`.

Both generated paths are ignored by Git and can be recreated from source.
