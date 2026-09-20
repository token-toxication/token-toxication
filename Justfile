set windows-shell := ["C:/Program Files/Git/bin/bash.exe", "-c"]

# List available recipes
default:
    @just --list

openapi_nexus := env_var_or_default("OPENAPI_NEXUS", "openapi-nexus")

# ---------- Rust ----------

# Build the Rust workspace
build:
    cargo build --workspace

# Check Rust formatting
fmt-check:
    cargo fmt --all -- --check

# Apply Rust formatting
fmt:
    cargo fmt --all

# Run clippy
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# Run Rust tests
test:
    cargo test --workspace

# Run the backend locally
dev-server:
    cargo run -p token-toxication-server --bin token-toxication-server

# Generate the OpenAPI JSON document from utoipa
openapi-generate:
    cargo run -p token-toxication-server --bin token-toxication-server -- generate-openapi --output openapi/token-toxication.openapi.json

# Generate the admin TypeScript SDK from the OpenAPI document
sdk-generate: openapi-generate
    rm -rf apps/admin/src/generated/token-toxication
    {{openapi_nexus}} generate \
        --input openapi/token-toxication.openapi.json \
        --generators typescript-fetch \
        --output apps/admin/src/generated/token-toxication \
        --config openapi-nexus.toml \
        --generator-config typescript-fetch.package_name=@token-toxication/admin-api
    cd apps/admin && vp format --no-error-on-unmatched-pattern src/generated/token-toxication

# ---------- Frontend ----------

# Install admin UI dependencies with Vite+
ui-install:
    cd apps/admin && vp install

# Verify that every translatable admin string is present in the locale catalogs.
ui-i18n-check:
    rm -rf apps/admin/.locales-snapshot
    cp -R apps/admin/src/locales apps/admin/.locales-snapshot
    cd apps/admin && vp exec lingui extract --clean
    diff -ru apps/admin/.locales-snapshot apps/admin/src/locales || (rm -rf apps/admin/.locales-snapshot && echo "Lingui catalogs are stale. Commit the extracted catalog changes." >&2 && exit 1)
    rm -rf apps/admin/.locales-snapshot

# Run Vite+ frontend checks
ui-check:
    cd apps/admin && vp check

# Run focused frontend tests
ui-test:
    cd apps/admin && vp test --run

# Opt-in real Codex client test against a local mock; pass an absolute binary path.
codex-compat codex_bin: build
    cd apps/admin && TT_CODEX_BIN={{quote(codex_bin)}} TT_RELAY_BIN={{quote(justfile_directory() / "target/debug/token-toxication-server")}} vp test --run src/admin-ui/codex-client.integration.test.ts

# Build admin UI
ui-build:
    cd apps/admin && vp build

# Serve admin UI in development mode
ui-dev:
    cd apps/admin && vp dev

# ---------- CI ----------

# Run the full local CI pipeline
ci: fmt-check clippy test sdk-generate ui-i18n-check ui-check ui-test ui-build

# ---------- Deployment ----------

# Build release binary and package deployment bundle into a single folder
package out_dir="dist-deploy": ui-build
    cargo build --release -p token-toxication-server --bin token-toxication-server
    rm -rf "{{out_dir}}"
    mkdir -p "{{out_dir}}"
    [ -f "target/release/token-toxication-server.exe" ] && cp target/release/token-toxication-server.exe "{{out_dir}}/" || true
    [ -f "target/release/token-toxication-server" ] && cp target/release/token-toxication-server "{{out_dir}}/" || true
    [ -f ".env.example" ] && cp .env.example "{{out_dir}}/.env.example" && cp .env.example "{{out_dir}}/.env" || true
    echo "Package ready in: {{out_dir}}/"

