#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
engine="${CONTAINER_ENGINE:-}"
if [[ -z "$engine" ]]; then
  if command -v docker >/dev/null && docker compose version >/dev/null 2>&1; then
    engine=docker
  else
    engine=podman
  fi
fi
if [[ "${NATIVE_APP:-0}" == 1 ]]; then
  cargo_bin="${CARGO_BIN:-cargo}"
  if "$cargo_bin" --version | grep -q huacnlee; then
    echo "Native app build requires upstream cargo; set CARGO_BIN explicitly." >&2
    exit 1
  fi
  "$cargo_bin" build --locked -j 4 -p server --manifest-path ../../Cargo.toml \
    --target-dir "$PWD/artifacts/target"
  set -- --native-app "$@"
fi
exec "${PYTHON_BIN:-python3}" run.py --engine "$engine" "$@"
