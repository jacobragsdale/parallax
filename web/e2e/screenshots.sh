#!/usr/bin/env bash
# Runs the screenshot tests in the Playwright image whose version matches the installed @playwright/test, against the
# reference server on this machine, so that fonts and text rendering are the same wherever they run. Starts the server
# if it is not already running. Arguments go to `playwright test`, such as --update-snapshots.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"
version="$(node -p "require('./web/node_modules/@playwright/test/package.json').version")"
health="http://127.0.0.1:8791/health"
if ! curl -sf "$health" >/dev/null; then
  target/release/parallax-server --config examples/experiment.toml >/dev/null 2>&1 &
  server=$!
  trap 'kill "$server"' EXIT
  curl -sf --retry 120 --retry-delay 1 --retry-all-errors "$health" >/dev/null
fi
docker run --rm --network host --user "$(id -u):$(id -g)" -e HOME=/tmp -e CI="${CI:-}" \
  -v "$root:$root" -w "$root/web" "mcr.microsoft.com/playwright:v$version-noble" \
  node_modules/.bin/playwright test -c e2e/playwright.config.ts screenshots.spec "$@"
