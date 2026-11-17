#!/usr/bin/env bash
# Exercise scripts/publish-release.py against a real (local, throw-away) registry:
# absent -> push -> same -> a second push is refused -> another revision is refused.
#
#   scripts/test-publish-local-registry.sh <local-image>
#
# <local-image> must carry the org.opencontainers.image.revision label (the Dockerfile's
# EMUMET_REVISION build argument). Nothing is pushed anywhere but to the local registry container,
# which is removed on exit (only the container this run created).
set -euo pipefail

IMAGE="${1:?usage: $0 <local-image>}"
ENGINE="${CONTAINER_ENGINE:-docker}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REGISTRY_IMAGE="docker.io/library/registry:2@sha256:a3d8aaa63ed8681a604f1dea0aa03f100d5895b6a58ace528858a7b332415373"
SUFFIX="$(date +%s)-$$"
NAME="emumet-publish-test-registry-${SUFFIX}"
WORK="$(mktemp -d)"
VERSION="2026.11.17"

log() { printf '[publish-test] %s\n' "$*"; }
die() { printf '[publish-test] FAIL: %s\n' "$*" >&2; exit 1; }

cleanup() {
  "$ENGINE" rm -f "$NAME" >/dev/null 2>&1 || true
  [ -n "${TAGGED:-}" ] && "$ENGINE" rmi "$TAGGED" >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

REVISION="$("$ENGINE" image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$IMAGE")"
case "$REVISION" in
  *[!0-9a-f]* | "") die "the image has no 40-hex revision label (got '${REVISION}')" ;;
esac
[ "${#REVISION}" = 40 ] || die "the revision label is not a full SHA: ${REVISION}"

"$ENGINE" run -d --name "$NAME" -p 127.0.0.1::5000 "$REGISTRY_IMAGE" >/dev/null
PORT="$("$ENGINE" port "$NAME" 5000/tcp | head -n1 | sed 's/.*://')"
REGISTRY="127.0.0.1:${PORT}"
for _ in $(seq 1 30); do
  curl -fsS "http://${REGISTRY}/v2/" >/dev/null 2>&1 && break
  sleep 1
done
curl -fsS "http://${REGISTRY}/v2/" >/dev/null || die "the local registry did not come up"
TAGGED="${REGISTRY}/shuttlepub/emumet:${VERSION}"

publish() {
  python3 "$ROOT/scripts/publish-release.py" "$1" --registry "$REGISTRY" --insecure-http \
    --version "$VERSION" --revision "${REVISION_ARG:-$REVISION}" "${@:2}"
}

log "1. the tag is absent"
publish classify --github-output "$WORK/out1" >"$WORK/log1" 2>&1 || { cat "$WORK/log1"; die "classify failed on an empty registry"; }
grep -qx 'state=absent' "$WORK/out1" || { cat "$WORK/log1"; die "expected state=absent"; }

log "2. push the smoke-tested local image"
publish push --local-image "$IMAGE" --github-output "$WORK/out2" >"$WORK/log2" 2>&1 || { cat "$WORK/log2"; die "push failed"; }
grep -qx 'state=same' "$WORK/out2" || die "push did not end in state=same"
DIGEST="$(sed -n 's/^digest=//p' "$WORK/out2")"
case "$DIGEST" in sha256:????????????????????????????????????????????????????????????????) ;; *) die "bad digest '${DIGEST}'" ;; esac
REGISTRY_DIGEST="$(curl -fsSI -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' \
  "http://${REGISTRY}/v2/shuttlepub/emumet/manifests/${VERSION}" | tr -d '\r' | sed -n 's/^[Dd]ocker-[Cc]ontent-[Dd]igest: //p')"
[ "$REGISTRY_DIGEST" = "$DIGEST" ] || die "the reported digest ${DIGEST} is not the registry's ${REGISTRY_DIGEST}"

log "3. classify now says same, with the same digest"
publish classify --github-output "$WORK/out3" >"$WORK/log3" 2>&1 || { cat "$WORK/log3"; die "classify failed after the push"; }
grep -qx 'state=same' "$WORK/out3" || die "classify after push does not say same"
grep -qx "digest=${DIGEST}" "$WORK/out3" || die "classify after push reports another digest"

log "4. a second push is refused (never overwrite)"
if publish push --local-image "$IMAGE" >"$WORK/log4" 2>&1; then die "a second push was accepted"; fi
grep -q 'appeared before the push' "$WORK/log4" || { cat "$WORK/log4"; die "unexpected reason for refusing the second push"; }

log "5. another revision is refused"
if REVISION_ARG="$(printf '%040d' 1)" publish classify >"$WORK/log5" 2>&1; then die "a different revision was accepted"; fi
grep -q 'refusing to overwrite' "$WORK/log5" || { cat "$WORK/log5"; die "unexpected reason for refusing the other revision"; }

log "ALL LOCAL-REGISTRY PUBLISH CHECKS PASSED (${TAGGED}@${DIGEST})"
