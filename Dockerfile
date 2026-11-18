# Emumet server image (linux/amd64, `server` binary only).
#
# Build:  docker build -t emumet:local .
# Check:  scripts/smoke-container.sh emumet:local
# Docs:   docs/container-runtime.md, docs/releasing.md
#
# The base images are pinned by digest (the tag is kept for readability). The Rust tag MUST equal
# `rust-toolchain.toml` (checked by scripts/check-container-toolchain.py in CI). To update a base
# image, see "Updating the base images" in docs/container-runtime.md.

# ---- builder ----------------------------------------------------------------------------------
FROM docker.io/rust:1.97.0-bookworm@sha256:8fa55b2f3ddf97471ab6a767bfa3f37e6bad0986ba823e75fea57e2a2a5c3073 AS build

WORKDIR /src

# Only what the build needs is copied (no .env, keys, logs, .git, target; see .dockerignore).
# Changing a migration changes the embedded `sqlx::migrate!` set, so `migrations` is copied
# before the build and invalidates it. `target` is intentionally not a cache mount: a stale
# cache must never hide a changed migration.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY kernel ./kernel
COPY application ./application
COPY driver ./driver
COPY server ./server
COPY migrations ./migrations

# The compiler that is actually in the builder must be the one `rust-toolchain.toml` names (a
# mismatch would make rustup try to download a toolchain, or silently build with another one).
RUN test "$(rustc --version | cut -d" " -f2)" = "$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)"

# No DB or secret is needed: the migrations are embedded at compile time, no `query!` macro
# talks to a database, and neither `test-mode` nor `--all-features` is enabled.
RUN cargo build --locked --release -p server --bin server

# ---- runtime ----------------------------------------------------------------------------------
FROM docker.io/debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc

# CalVer (without the leading `v`) and the full commit SHA are supplied by the release workflow;
# the defaults mark a local build. They are declared late so they never invalidate the build stage.
ARG EMUMET_VERSION=dev
ARG EMUMET_REVISION=unknown

LABEL org.opencontainers.image.title="emumet" \
      org.opencontainers.image.description="Emumet server" \
      org.opencontainers.image.source="https://github.com/ShuttlePub/Emumet" \
      org.opencontainers.image.version="${EMUMET_VERSION}" \
      org.opencontainers.image.revision="${EMUMET_REVISION}"

# libssl3: linked by the binary (openssl-sys). ca-certificates: TLS to Hydra / Kratos / S3 / SMTP.
RUN apt-get update \
    && apt-get install -y --no-install-recommends libssl3 ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 emumet \
    && useradd --uid 10001 --gid 10001 --no-create-home --shell /usr/sbin/nologin emumet

COPY --from=build /src/target/release/server /app/server

WORKDIR /app
USER 10001:10001

# Documentation only: the server binds `HTTP_LISTEN_ADDR` (default 0.0.0.0:8080).
EXPOSE 8080
STOPSIGNAL SIGTERM

# exec form: the server is PID 1 and receives SIGTERM directly.
ENTRYPOINT ["/app/server"]
