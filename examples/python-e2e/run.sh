#!/usr/bin/env bash
# Generate the Python client from the live API and drive a 100-home
# fleet end to end. Requires: cargo, npx, java, python3.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PORT="${BATSIM_PORT:-18099}"
BASE="http://127.0.0.1:${PORT}"
CLIENT_DIR="${ROOT}/clients/python"
WORK="$(mktemp -d)"
trap 'rc=$?; if [[ ${rc} -ne 0 && -f "${WORK}/server.log" ]]; then echo "== server.log ==" >&2; cat "${WORK}/server.log" >&2; fi; rm -rf "${WORK}"; [[ -n "${SERVER_PID:-}" ]] && kill "${SERVER_PID}" 2>/dev/null || true' EXIT

echo "== building batsim"
cargo build -p batsim-server --quiet

echo "== starting server on :${PORT}"
"${ROOT}/target/debug/batsim" --port "${PORT}" > "${WORK}/server.log" 2>&1 &
SERVER_PID=$!
healthy=0
for _ in $(seq 1 50); do
  if curl -sf "${BASE}/v1/system/health" > /dev/null; then healthy=1; break; fi
  sleep 0.2
done
if [[ "${healthy}" -ne 1 ]]; then
  echo "server did not become healthy on :${PORT}" >&2
  exit 1
fi

echo "== generating python client"
rm -rf "${CLIENT_DIR}"
npx --yes @openapitools/openapi-generator-cli generate \
  -i "${BASE}/openapi.json" \
  -g python -o "${CLIENT_DIR}" \
  --additional-properties=packageName=batsim_client > /dev/null

echo "== installing client into a venv"
python3 -m venv "${WORK}/venv"
"${WORK}/venv/bin/pip" install --quiet --upgrade pip
"${WORK}/venv/bin/pip" install --quiet "${CLIENT_DIR}"

echo "== driving a 100-home fleet"
BATSIM_URL="${BASE}" "${WORK}/venv/bin/python" "${ROOT}/examples/python-e2e/e2e.py"
