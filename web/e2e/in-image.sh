#!/usr/bin/env bash
# Runs Playwright in the Docker image of the installed @playwright/test version, against a reference server this
# script starts on the host, so that the browser, fonts and text rendering are the same on every machine. It builds the
# wasm worker, the demo and the server first, unless PARALLAX_PREBUILT=1 says they are built already, as in CI.
# Arguments go to `playwright test`, such as spec names or --update-snapshots.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
web="$root/web"
read -ra command <<<"$(node -p "require('$web/e2e/reference-server.json').command")"
health="$(node -p "require('$web/e2e/reference-server.json').health")"
version="$(node -p "require('$web/node_modules/@playwright/test/package.json').version")"

# A server already running may be serving an older build, which the tests would then pass or record.
if curl -sf "$health" >/dev/null; then
  echo "A server already answers at $health. Stop it first: these tests run against the build this script starts." >&2
  exit 1
fi
if [ "${PARALLAX_PREBUILT:-}" != "1" ]; then
  (cd "$web" && pnpm build:wasm && pnpm build)
  (cd "$root" && cargo build --release --locked -p parallax-server)
fi

log="$(mktemp)"
cd "$root"
"${command[@]}" >"$log" 2>&1 &
server=$!
trap 'kill "$server" 2>/dev/null || true; rm -f "$log"' EXIT
for _ in $(seq 120); do
  curl -sf "$health" >/dev/null && break
  if ! kill -0 "$server" 2>/dev/null; then
    echo "The reference server exited before it was ready:" >&2
    cat "$log" >&2
    exit 1
  fi
  sleep 1
done
curl -sf "$health" >/dev/null || { echo "The reference server did not answer at $health within 120 s:" >&2; cat "$log" >&2; exit 1; }

docker run --rm --network host --user "$(id -u):$(id -g)" -e HOME=/tmp -e CI="${CI:-}" -e PARALLAX_PINNED_IMAGE=1 \
  -v "$root:$root" -w "$web" "mcr.microsoft.com/playwright:v$version-noble" \
  node_modules/.bin/playwright test -c e2e/playwright.config.ts "$@" || {
  status=$?
  echo "The reference server's log:" >&2
  tail -n 50 "$log" >&2
  exit "$status"
}
