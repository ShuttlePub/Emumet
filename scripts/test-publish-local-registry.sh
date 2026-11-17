#!/usr/bin/env bash
# Exercise scripts/publish-release.py against a real (local, throw-away) registry:
# absent -> push -> same -> a copy pulled by digest verifies -> a copy with the same layers but another
# Entrypoint is refused -> a second push is refused -> another revision is refused.
#
#   scripts/test-publish-local-registry.sh <local-image>
#
# <local-image> must carry the org.opencontainers.image.revision label (the Dockerfile's
# EMUMET_REVISION build argument). Nothing is pushed anywhere but to the local registry container,
# which is removed on exit (only the container this run created).
#
# Docker only: scripts/publish-release.py runs the `docker` CLI (save, tag, push), so a different
# CONTAINER_ENGINE is refused instead of being silently ignored by half of the test.
set -euo pipefail

IMAGE="${1:?usage: $0 <local-image>}"
ENGINE="${CONTAINER_ENGINE:-docker}"
[ "$ENGINE" = docker ] || { echo "[publish-test] FAIL: only CONTAINER_ENGINE=docker is supported (got '$ENGINE')" >&2; exit 1; }
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
  if [ -n "${TAGGED:-}" ]; then "$ENGINE" rmi "$TAGGED" >/dev/null 2>&1 || true; fi
  if [ -n "${PULLED:-}" ]; then "$ENGINE" rmi "$PULLED" >/dev/null 2>&1 || true; fi
  if [ -n "${VARIANT:-}" ]; then "$ENGINE" rmi "$VARIANT" >/dev/null 2>&1 || true; fi
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

log "3b. a copy pulled by digest verifies against the registry"
PULLED="${REGISTRY}/shuttlepub/emumet@${DIGEST}"
"$ENGINE" rmi "$TAGGED" >/dev/null 2>&1 || true
"$ENGINE" pull -q "$PULLED" >/dev/null || die "cannot pull the pushed image by digest"
publish verify-local --local-image "$PULLED" --expected-digest "$DIGEST" >"$WORK/log3b" 2>&1 \
  || { cat "$WORK/log3b"; die "verify-local failed for the image pulled by digest"; }
log "3c. the same layers with another Entrypoint (config only) are refused"
VARIANT="emumet-publish-test-variant:${SUFFIX}"
printf 'FROM %s\nENTRYPOINT ["/app/server", "--changed"]\n' "$IMAGE" | "$ENGINE" build -q -t "$VARIANT" - >/dev/null \
  || die "cannot build the config-only variant"
[ "$("$ENGINE" image inspect --format '{{json .RootFS.Layers}}' "$VARIANT")" = "$("$ENGINE" image inspect --format '{{json .RootFS.Layers}}' "$IMAGE")" ] \
  || die "the variant does not have the same layers; the test would prove nothing"
if publish verify-local --local-image "$VARIANT" --expected-digest "$DIGEST" >"$WORK/log3c" 2>&1; then
  die "an image with another Entrypoint was accepted"
fi
grep -q 'is not the config' "$WORK/log3c" || { cat "$WORK/log3c"; die "unexpected reason for refusing the variant"; }
log "3d. another expected digest is refused"
if publish verify-local --local-image "$PULLED" --expected-digest "sha256:$(printf '%064d' 7)" >"$WORK/log3d" 2>&1; then
  die "a different expected digest was accepted"
fi

log "4. a second push is refused (never overwrite)"
if publish push --local-image "$IMAGE" >"$WORK/log4" 2>&1; then die "a second push was accepted"; fi
grep -q 'appeared before the push' "$WORK/log4" || { cat "$WORK/log4"; die "unexpected reason for refusing the second push"; }

log "5. another revision is refused"
if REVISION_ARG="$(printf '%040d' 1)" publish classify >"$WORK/log5" 2>&1; then die "a different revision was accepted"; fi
grep -q 'refusing to overwrite' "$WORK/log5" || { cat "$WORK/log5"; die "unexpected reason for refusing the other revision"; }

log "ALL LOCAL-REGISTRY PUBLISH CHECKS PASSED (${TAGGED}@${DIGEST})"
