#!/usr/bin/env bash
# No implicit teardown: run the dedicated compose down separately (never down -v).
set -euo pipefail
cd "$(dirname "$0")/../.."
engine="${CONTAINER_ENGINE:-docker}"
cargo_bin="${CARGO_BIN:-cargo}"
python3 e2e/registration-enforcement/prepare.py
compose=("$engine" compose -p emumet-registration-enforcement-e15d -f e2e/registration-enforcement/compose.yml)
"${compose[@]}" up -d --wait postgres
"${compose[@]}" exec -T postgres psql -U guard -d emumet -v ON_ERROR_STOP=1 -c "CREATE DATABASE kratos"
"${compose[@]}" run --rm -T kratos-migrate
"${compose[@]}" up -d kratos
python3 e2e/registration-enforcement/wait.py
export DATABASE_URL=postgres://guard:fixture-only@127.0.0.1:15448/emumet
export REGISTRATION_KRATOS_E2E=1 REGISTRATION_E2E_ENGINE="$engine"
"$cargo_bin" test -p server real_kratos_d15_q01_through_q11 -- --ignored --nocapture --test-threads=1 2>&1 | tee e2e/registration-enforcement/result.log
grep -Eq 'test result: ok\. 1 passed; 0 failed; 0 ignored;' e2e/registration-enforcement/result.log
