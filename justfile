# batsim task runner. `just --list` shows everything.

# fmt + clippy + cargo check
check:
    cargo fmt --all -- --check
    cargo clippy --workspace --all-targets -- -D warnings
    cargo check --workspace

# unit + integration tests
test:
    cargo test --workspace

# regenerate physics goldens (review the diff!)
golden:
    INSTA_UPDATE=always cargo test -p batsim-core --test golden

# regenerate the vendored OpenAPI document and diff against HEAD
spec:
    cargo run -p batsim-server -- --dump-openapi > api/openapi.json
    git diff --stat api/openapi.json

# contract tests against a fixture server
contract:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build -p batsim-server --quiet
    ./target/debug/batsim --port 18097 & SRV=$!
    trap 'kill $SRV 2>/dev/null || true' EXIT
    for i in $(seq 1 50); do curl -sf http://127.0.0.1:18097/v1/system/health >/dev/null && break; sleep 0.2; done
    curl -sf http://127.0.0.1:18097/v1/system/health >/dev/null
    schemathesis run http://127.0.0.1:18097/openapi.json --checks all --request-timeout 60 \
      --exclude-path /v1/telemetry/stream --exclude-path /v1/telemetry/ws

# generated-client end-to-end (100-home fleet)
e2e:
    examples/python-e2e/run.sh

# everything CI runs locally
ci: check test spec contract e2e
