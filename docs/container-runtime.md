# Container runtime

`ghcr.io/shuttlepub/emumet:<CalVer>` runs the `server` binary only (linux/amd64, Debian bookworm
slim, user/group `10001:10001`, `ENTRYPOINT ["/app/server"]`, `STOPSIGNAL SIGTERM`). It contains
no compiler, source, migration SQL, recovery CLI (`profile-cutover-recovery`) or curl. Releases
are described in [releasing.md](releasing.md).

## Start-up and what you must provide

* **Database**: `DATABASE_URL` (or the five `DATABASE_*` variables). Migrations are embedded in
  the binary and applied at start-up; an unreachable database is a start-up failure (non-zero
  exit). The migration step itself is never interrupted.
* **Master key**: a file read as the container user. The server reads
  `/run/secrets/master-key-password`, falling back to `./master-key-password` (`/app`). The file
  must not be readable by group/other (mode `0400` or `0600`; any `0o077` bit is rejected) and must
  be readable by the user the container runs as (UID 10001 by default). Mount it read-only; never
  bake it into an image or pass it as an environment variable. With a plain Docker named volume (as
  `scripts/smoke-container.sh` does) create the file inside the volume with `chown 10001:10001` and
  `chmod 0400` using a throw-away container. Three cautions:
  1. **A wrong key file is not necessarily detected at start-up.** The file is read when a key is
     first needed (creating or using an account/profile key), not unconditionally when the server
     starts. A missing, unreadable, empty or too-permissive file can therefore stay unnoticed until
     the first such operation. The smoke test only proves that the default user can read a `0400`
     file at OS level, not that the application uses the key correctly.
  2. **If you override the UID, make the file owner match it.** `runAsUser` and similar options do
     not change who owns the file, and because group/other permission bits are rejected, granting
     access through a group (`fsGroup`, mode `0440`) is not an alternative: the file must be owned
     by the UID that actually runs the server and be `0400`/`0600`.
  3. **Mounting a Secret directly (for example a Kubernetes Secret volume) is unverified.** Its
     ownership and mode semantics were not tested with this image. Verify that the server really
     reads the key (an operation that needs it, such as creating a profile) in the target
     environment before the production placement.
* **Health**: `GET /healthz` returns `200 ok\n` (`text/plain`, `Cache-Control: no-store`) without
  authentication. It only tells that the HTTP server is up: it never touches the database, Ory,
  S3 or the key, so it is suitable as a liveness probe, but not as proof of readiness for work.
  It is not listed in `openapi.json`.
* **Root filesystem**: the image runs on a read-only root with every capability dropped. By
  default no file is written. `EMUMET_FILE_LOG_ENABLED=true` needs a writable `/app/logs`.
* **Stopping**: SIGTERM and SIGINT take the same path: the listener stops accepting, in-flight
  requests finish, then the background workers are stopped in the existing order (the webhook
  worker may wait up to 20 seconds). Give the container a **termination grace period of at least
  30 seconds** (`docker stop -t 30`, Kubernetes `terminationGracePeriodSeconds: 30` or more). The
  30 seconds is a recommendation, **not a guaranteed upper bound**: in-flight requests have no
  deadline of their own, and a stop request that arrives during the database migration is honoured
  only after the migration (which is never interrupted) finishes. If the orchestrator's grace period
  runs out first, the process is killed (SIGKILL) and the shutdown was not clean. Exit code on a
  clean stop is 0.

## Logging

stdout is the log of record. The built-in default filter is
`driver=debug,server=debug,tower_http=debug,hyper=debug,sqlx=debug`, which is a development setting:
`sqlx=debug` prints every SQL statement text and the projection workers poll continuously, so the
output is very large. For production set, for example:

```
RUST_LOG=info,sqlx=warn,hyper=warn,tower_http=info
```

(`RUST_LOG` is read from the process environment only: it is read before `.env` is loaded. The output
level is also capped at DEBUG.)

`EMUMET_FILE_LOG_ENABLED` (new): unset or `false` writes no file; `true` additionally writes
`./logs/debug.log.<date>` (a development convenience, stdout stays on); any other value (empty,
`TRUE`, `1`) or an unwritable `./logs` is a start-up error, never a silent fallback. It is read from the
process environment only, never from `.env`. The file is split per day but **old files are never
deleted automatically**, and the file log is not limited by `RUST_LOG`: it records every target at DEBUG and above (including SQL text), so it grows quickly. If you enable it, plan the
retention period and the disk capacity of the log directory yourself (rotation/cleanup outside the
container, or a size-limited volume); on a read-only root it needs a writable `/app/logs`.

## Environment variables

Extracted from the code at the release base commit with:

```sh
git grep -nE 'env::var|dotenvy::var|source\.get|env_or|const .*ENV_|"([A-Z][A-Z0-9]*_)+[A-Z0-9]+"' <base> \
  -- kernel/src application/src driver/src server/src
```

(the result and the classification are also recorded in the pull request that introduced this
file). Values read through `dotenvy` also come from a `.env` file in the working directory; the
image has none, so provide them in the container environment.

### Required or practically required in production

| Variable | Default in code | Meaning |
| --- | --- | --- |
| `DATABASE_URL` (or `DATABASE_HOST`, `DATABASE_PORT`, `DATABASE_USER`, `DATABASE_PASSWORD`, `DATABASE_NAME`) | none (start-up error) | PostgreSQL. `DATABASE_URL` wins when both are set. |
| `HYDRA_ISSUER_URL` | `http://localhost:4444` (warns) | Issuer of the access tokens; JWKS is fetched from it at start-up. |
| `EXPECTED_AUDIENCE` | `emumet` (warns) | Required JWT audience. (`.env.example` sets `account` for the Ory development stack; the code default is `emumet`.) |
| `HYDRA_ADMIN_URL` | `http://localhost:4445` | Hydra admin API (login/consent). |
| `KRATOS_PUBLIC_URL`, `KRATOS_ADMIN_URL` | `http://localhost:4433`, `http://localhost:4434/` | Kratos. |
| `KETO_READ_URL`, `KETO_WRITE_URL` | `http://localhost:4466`, `http://localhost:4467` | Keto. |
| `PUBLIC_BASE_URL` | `http://localhost:8080` | Public origin used to build ActivityPub IDs and signatures. |
| `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET`, `S3_ACCESS_KEY`, `S3_SECRET_KEY`, `MEDIA_PUBLIC_BASE_URL` | development values (`http://localhost:9000`, `us-east-1`, `emumet-media`, `emumetdevelop`, `emumet-develop-secret`, ...) | Media storage. The secret default is a development credential: always set your own. |
| `WORKER_ID` | `0` | Snowflake worker ID: an integer that must be unique per running instance. A missing or unparsable value silently becomes `0`. Read from the process environment only (before `.env` is loaded). |
| `CORS_ALLOWED_ORIGINS` | unset = permissive (any origin) | Comma-separated origins; `*` or unset is `CorsLayer::permissive()`. Set it explicitly in production. |

### Optional

| Variable | Default | Meaning |
| --- | --- | --- |
| `HTTP_LISTEN_ADDR` | `0.0.0.0:8080` | Listen address. |
| `RUST_LOG` | see Logging | Log filter. |
| `EMUMET_FILE_LOG_ENABLED` | off | See Logging. |
| `PROJECTION_POLL_INTERVAL_MS` | `100` | Projection worker poll. |
| `PROFILE_DELIVERY_POLL_INTERVAL_MS` | `2000` | Profile delivery worker poll. |
| `REGISTRATION_WEBHOOK_POLL_INTERVAL_MS` | `2000` | Webhook worker poll. |
| `REGISTRATION_WEBHOOK_ENCRYPTION_KEY` / `_FILE` | see the linked document | Needed to create or replace webhook secrets. See `docs/registration-gate-webhook.md`. |
| `REGISTRATION_WEBHOOK_URL`, `REGISTRATION_WEBHOOK_SECRET` / `_FILE` | unset (feature not used) | Legacy single webhook destination. |
| `REGISTRATION_TURNSTILE_*` (`ENABLED`, `SITE_KEY`, `SECRET` / `SECRET_FILE`, `EXPECTED_HOSTNAME`, `EXPECTED_ACTION`, `TOKEN_MAX_AGE_SECS`) | see the linked document | Cloudflare Turnstile. See `docs/registration-protection-readiness.md`. |
| `REGISTRATION_EMAIL_*`, `REGISTRATION_SMTP_*` (`ENABLED`, `FROM`, `CANARY_ADDRESS`, `SMTP_HOST`, `_PORT`, `_TLS`, `_USERNAME`, `_PASSWORD` / `_PASSWORD_FILE`) | see the linked document | Email proof. Same document. |
| `REGISTRATION_EMAIL_TRUSTED_PROXY_IPS` | empty | Reverse proxies whose forwarded address is trusted. |
| `REGISTRATION_PROTECTION_SUCCESS_TTL_SECS`, `REGISTRATION_PROTECTION_PROBE_MAX_AGE_SECS`, `REGISTRATION_PROOF_REUSE_WINDOW_SECS`, `REGISTRATION_EMAIL_MAX_PER_SOURCE_HOUR` | built-in policy | Positive integers only; anything else is ignored. |
| `REGISTRATION_ENFORCEMENT_ENABLED` | `false` | Registration hook listener (preparation only). Needs a hook secret (`REGISTRATION_HOOK_SECRET` or `_FILE`). Also `REGISTRATION_HOOK_LISTEN` (default `127.0.0.1:8081`: unreachable from outside the container unless changed), `REGISTRATION_HOOK_ALLOWED_PEERS` (`127.0.0.1,::1`), `REGISTRATION_PROTECTION_POLL_SECS` (`30`). |
| `REGISTRATION_ENFORCEMENT_PROBE_TARGETS`, `REGISTRATION_ENFORCEMENT_PROBE_PERIOD_SECS` | Kratos URLs above, `30` | Kratos enforcement probe. |

### Read by the code but unused by the server

`REDIS_URL`, `REDIS_HOST`: `driver/src/database/redis.rs` can read them, but the server never
opens a Redis connection. No Redis is needed.

### Test only — never set in production

`AP_TEST_ACCEPT_INVALID_CERTS`, `AP_TEST_ALLOWED_FETCH_HOSTS` (ActivityPub fetch relaxations),
`EMUMET_TEST_MODE_TOKEN` (needs the `test-mode` feature, which the image is built **without**; the
`/__test__` routes do not exist in it), `REGISTRATION_E2E_ENGINE`, `REGISTRATION_KRATOS_E2E`,
`REGISTRATION_E15C_UNSET_FOR_TEST`, `PEER_BASE_URL` and the `EMUMET_E2E_*`/`ICESHRIMP_BASE_URL`
variables of the e2e stack.

### Tools not in the image

`profile-cutover-recovery` (it reads `DATABASE_URL` and `WORKER_ID`) is a separate binary that is
deliberately not shipped in this image.
