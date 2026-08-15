# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## intent-cli workflow

このリポジトリは intent-cli の **design host** (domain: `emumet`, target: `ShuttlePub/Emumet`) でもある。

- 意図ツリー: `intents/emumet/` (identity / product / features / decisions / clarifications / packets)
- packet (issue の設計契約): `.intent-cli/issues/<unit>/`
- 今後の作業の順序付き backlog: `intents/emumet/packets/backlog.md`
- ガイダンスはコピーせず CLI から読むこと: `intent-cli guide model --format json`, `intent-cli guide commands list --format json`
- ラベル遷移・issue publish は `intent-cli automation` / `intent-cli issue` 経由で行い、queue-state 等を直接編集しない

## Project Overview

Emumet is an Account Service for ShuttlePub, implementing Event Sourcing with CQRS pattern. The name derives from EMU (Extravehicular Mobility Unit) + Helmet.

## Git Commit Convention

コミットメッセージには [gitmoji](https://gitmoji.dev/) をプレフィックスとして付けること。

形式: `:gitmoji: Subject line`

例:
- `:sparkles: Add user registration endpoint` (新機能)
- `:bug: Fix race condition in event store` (バグ修正)
- `:recycle: Refactor DependOn trait hierarchy` (リファクタリング)
- `:memo: Update API spec` (ドキュメント)
- `:white_check_mark: Add integration tests for profile` (テスト)
- `:zap: Optimize batch query for profiles` (パフォーマンス)
- `:wrench: Update environment config` (設定)

## Build & Development Commands

```bash
# Build
cargo build

# Run tests (requires DATABASE_URL environment variable)
cargo test

# Run single test
cargo test <test_name>

# Run server
cargo run -p server
```

## Required Services

Use `podman-compose up` (or `docker-compose up`) to start all required services:

```bash
podman-compose up -d
```

This starts: PostgreSQL, Redis, Ory Kratos, and Ory Hydra.

### Manual startup (alternative)

#### PostgreSQL
```bash
podman run --rm --name emumet-postgres -e POSTGRES_PASSWORD=develop -p 5432:5432 docker.io/postgres
```
- User: postgres / Password: develop

#### Redis
Required for message queue (rikka-mq).

### Auth: Ory Kratos + Hydra

- **Kratos** (identity management): http://localhost:4433 (public), http://localhost:4434 (admin)
  - Self-service registration enabled
  - Identity schema: email + password
  - Test user: testuser@example.com / testuser
- **Hydra** (OAuth2/OIDC): http://localhost:4444 (public), http://localhost:4445 (admin)
  - Login/Consent Provider: Emumet server (GET /oauth2/login, GET/POST /oauth2/consent)
  - JWT issuer

Config files: `ory/kratos/`, `ory/hydra/`

## Environment Variables

Copy `.env.example` to `.env`:
- `DATABASE_URL` or individual `DATABASE_HOST`, `DATABASE_PORT`, `DATABASE_USER`, `DATABASE_PASSWORD`, `DATABASE_NAME`
- `HYDRA_ISSUER_URL` — Hydra public URL for JWT validation (default: http://localhost:4444/)
- `HYDRA_ADMIN_URL` — Hydra admin URL for Login/Consent API (default: http://localhost:4445/)
- `KRATOS_PUBLIC_URL` — Kratos public URL for session verification (default: http://localhost:4433/)
- `EXPECTED_AUDIENCE` — Expected JWT audience claim (default: account)
- `REDIS_URL` or `REDIS_HOST` — Redis connection for message queue

### Master Key Password

Account creation requires a master key password file for signing key encryption:
- Production: `/run/secrets/master-key-password`
- Development: `./master-key-password` (create manually, `chmod 600`)

## Architecture

### Workspace Structure (4 crates with dependency flow)

```
kernel → application → server
kernel → driver    → server
```

- **kernel**: Domain entities, interface traits (EventStore, ReadModel, Repository, *Query), Event Sourcing core, crypto trait composition (SigningKeyGenerator). Traits are exposed via logical `pub mod interfaces {}` block in `lib.rs` (not a physical directory).
- **application**: Use case services (Account CRUD use cases), projection projectors (tailing), DTOs
- **driver**: PostgreSQL implementations of kernel interfaces
- **server**: Axum HTTP server, JWT auth (Ory Hydra), OAuth2 Login/Consent Provider, route handlers, facade newtypes (server/src/api/), DI wiring (AppModule)

### CQRS + Event Sourcing Pattern

Two entity types exist in the codebase: **CQRS-migrated** and **legacy (Query/Modifier)**.

#### CQRS-migrated entities (Account, AuthAccount, Profile, Metadata)

Each CQRS entity has these components across layers:

```
Command Flow:
  REST handler → CommandProcessor (adapter)
    → EventStore.persist_and_transform() (kernel trait, driver impl)
    → EventApplier (kernel) → entity reconstruction
    → ReadModel.create/update() for immediate consistency (synchronous write)
    → ProjectionWorker (tailing projector) for guaranteed eventual consistency

Query Flow:
  REST handler → facade (server/src/api/) → *Query (kernel trait)
    → ReadModel.find_*() (kernel trait, driver impl)
```

**kernel** defines per-entity interface traits:
- `AccountEventStore` / `ProfileEventStore` / `MetadataEventStore` — event persistence + retrieval per entity-specific table
- `AccountReadModel` / `ProfileReadModel` / `MetadataReadModel` — projection reads + writes
- `AccountQuery` / `ProfileQuery` / `MetadataQuery` — read model facade traits (blanket impl over `DependOn*ReadModel`)
- `AuthAccountRepository` — unified CRUD interface for `AuthAccount` (migrated from Event Sourcing to plain CRUD in ADR 0006 Stage 6)

**driver** implements per-entity stores:
- `PostgresAccountEventStore` → `account_events` table
- `PostgresProfileEventStore` → `profile_events` table
- `PostgresMetadataEventStore` → `metadata_events` table
- `PostgresAccountReadModel` → `accounts` table
- `PostgresProfileReadModel` → `profiles` table
- `PostgresMetadataReadModel` → `metadatas` table
- `PostgresAuthAccountRepository` → `auth_accounts` table

**application** provides use case services and event appliers:
- `GetAccountUseCase` / `CreateAccountUseCase` / `UpdateAccountUseCase` / `DeactivateAccountUseCase` / `SuspendAccountUseCase` / `UnsuspendAccountUseCase` / `BanAccountUseCase` — Account CRUD + moderation orchestration via CommandProcessor/QueryProcessor
- `GetProfileUseCase` / `UpdateProfileUseCase` — Profile read/update (Profile is auto-created during Account creation)
- `GetMetadataUseCase` / `CreateMetadataUseCase` / `UpdateMetadataUseCase` / `DeleteMetadataUseCase` — Metadata CRUD
- `UpdateProfile` / `UpdateMetadata` — event appliers that replay events from EventStore, update ReadModel projections

#### Repository entities (Follow, RemoteAccount, Image, AuthHost)

These use the Repository pattern — a single trait combining read and write operations:
- `*Repository` traits in `kernel/src/repository/` — unified CRUD interface
- `Postgres*Repository` driver implementations in `driver/src/database/postgres/`
- Follow and RemoteAccount are pure CRUD (Event Sourcing removed)
- AuthHost and Image are pure CRUD (never had Event Sourcing)

### Key Patterns

**DependOn\* trait pattern**: Dependency injection via associated types. `DependOnFoo` provides `fn foo(&self) -> &Self::Foo`. Blanket impls auto-wire when dependencies are satisfied.

**impl_database_delegation! macro** (kernel/src/lib.rs): Delegates all database `DependOn*` traits from a wrapper type to a database field. Used by `AppModule` to wire `PostgresDatabase`.

**EventApplier trait** (kernel/src/event.rs): Reconstructs entity state from events. `fn apply(entity: &mut Option<Self>, event: EventEnvelope) -> Result<()>`. Entity becomes `None` on Deleted events.

**Optimistic concurrency control**: Commands carry `prev_version: Option<ExpectedVersion>`. `ExpectedVersion::Nothing` = must be first event, `ExpectedVersion::At(version)` = must match latest version. EventStore validates before persisting.

**Projection (tailing projector)**: `ProjectionWorker` polls event tables (100ms default), `*Projector` folds events per aggregate in version order with version-gated upserts and checkpoint tracking. Account projector cascade-deletes dependent Profile/Metadata rows; Profile/Metadata projectors skip fresh materialization when parent account is deleted.

### Auth Architecture

JWT validation middleware (`server/src/auth.rs`):
- OIDC Discovery → JWKS cache (with kid-miss re-fetch, rate-limited)
- Bearer token → RS256 validation → `Extension<AuthClaims>` inserted into request
- `AuthClaims` → `OidcAuthInfo` → `resolve_auth_account_id` (find-or-create AuthHost + AuthAccount)

OAuth2 Login/Consent Provider (`server/src/route/oauth2.rs`):
- GET /oauth2/login — Kratos session → Hydra login accept
- GET /oauth2/consent — skip check → redirect or show consent
- POST /oauth2/consent — accept/reject with scope validation

Value mapping: JWT `iss` → `AuthHost.url`, JWT `sub` (Kratos identity UUID) → `AuthAccount.client_id`

### Entity Structure

Entities use vodca macros (`References`, `Newln`, `Nameln`) and `destructure::Destructure` for field access.

Event Sourcing対象エンティティ (Account, AuthAccount, Profile, Metadata):
- ID type (i64 Snowflake-style, provides temporal ordering)
- Event enum with variants (Created, Updated, Deleted) + `Nameln` for event name serialization
- `EventApplier` implementation
- `CommandEnvelope` factory methods (e.g., `Account::create()`, `Account::delete()`)

純粋CRUDエンティティ (Follow, RemoteAccount, AuthHost, Image):
- ID type のみ。Event enum / EventApplier なし
- Repository パターンで直接 CRUD 操作

### Server DI Architecture

`AppModule` — single wiring root. Owns `PostgresDatabase` + crypto providers + HydraAdminClient + KratosClient. `impl_database_delegation!` wires all DB `DependOn*` traits; small handwritten impls for non-DB traits (crypto, permission, http_signing, config). Provides `hydra_admin_client()` and `kratos_client()` accessors.

**Facade newtypes** (server/src/api/): `AccountApi`, `AdminAccountApi`, `MeApi`, `OAuth2Api`, `ActivityPubApi`, `SigningApi`. Each wraps `AppModule` via `FromRef`, exposes only use-case methods + legitimate helpers. Facades do NOT implement `DependOn*` — route handlers cannot reach raw ports/executors (compile-time block).

### Testing

Database tests use `#[test_with::env(DATABASE_URL)]` attribute to skip when database is unavailable.

### Data Cleanup (after auth migration)

If migrating from Keycloak to Ory, truncate auth-related tables:
```sql
TRUNCATE auth_hosts, auth_accounts;
```
