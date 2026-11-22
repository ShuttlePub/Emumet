#!/usr/bin/env bash
# T1 post-relay E2E: real PostgreSQL + real Kratos/Hydra/Keto (upstream images, compose project
# emumet-post-relay-e2e), the Emumet server built from THIS source tree, and the release Dockerfile
# built locally for the operator checks. Nothing is published: no registry push, no tag, no release.
#
#   bash e2e/post-relay/run.sh                 # whole suite (about 15 minutes after the image build)
#   T1_KEEP_STACK=1 bash e2e/post-relay/run.sh # leave the stack up for debugging
#   T1_RELEASE_IMAGE=my:image ...              # reuse an already built local image
#   T1_FILTER=case01 ...                       # run a subset (the image is still required for case29)
set -euo pipefail
cd "$(dirname "$0")/../.."
compose=(docker compose -p emumet-post-relay-e2e -f e2e/post-relay/compose.yml)
result=e2e/post-relay/result.log

cleanup() {
  "${compose[@]}" logs --no-color > e2e/post-relay/stack.log 2>&1 || true
  if [[ "${T1_KEEP_STACK:-0}" != "1" ]]; then
    # Only this dedicated project (and its own volume) is removed.
    "${compose[@]}" down -v --remove-orphans >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

# The containers run as non-root users and read these bind-mounted configs.
chmod 644 e2e/post-relay/hydra.yml e2e/post-relay/kratos.yml

if [[ -z "${T1_RELEASE_IMAGE:-}" ]]; then
  T1_RELEASE_IMAGE=emumet-t1-release:local
  docker build --build-arg EMUMET_REVISION=t1-local -t "$T1_RELEASE_IMAGE" .
fi
export T1_RELEASE_IMAGE

"${compose[@]}" up -d
export DATABASE_URL=postgres://postgres:develop@127.0.0.1:15450/postgres

cargo test -p server --features test-mode --test e2e_post_relay -- --ignored --test-threads=1 ${T1_FILTER:-} 2>&1 | tee "$result"
grep -Eq 'test result: ok\. [0-9]+ passed; 0 failed; 0 ignored;' "$result"
