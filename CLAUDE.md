# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Emumet is an Account Service for ShuttlePub, implementing Event Sourcing with CQRS pattern. The name derives from EMU (Extravehicular Mobility Unit) + Helmet.

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

### PostgreSQL
```bash
podman run --rm --name emumet-postgres -e POSTGRES_PASSWORD=develop -p 5432:5432 docker.io/postgres
```
- User: postgres / Password: develop

### Redis
Required for message queue (rikka-mq).

### Keycloak
```bash
mkdir -p keycloak-data/h2
podman run --rm -it -v ./keycloak-data/h2:/opt/keycloak/data/h2:Z,U -v ./keycloak-data/import:/opt/keycloak/data/import:Z,U -p 18080:8080 -e KC_BOOTSTRAP_ADMIN_USERNAME=admin -e KC_BOOTSTRAP_ADMIN_PASSWORD=admin --name emumet-keycloak quay.io/keycloak/keycloak:26.1 start-dev --import-realm
```
- URL: http://localhost:18080
- Admin: admin / admin
- Realm: emumet, Client: myclient, User: testuser / testuser

## Environment Variables

Copy `.env.example` to `.env`:
- `DATABASE_URL` or individual `DATABASE_HOST`, `DATABASE_PORT`, `DATABASE_USER`, `DATABASE_PASSWORD`, `DATABASE_NAME`
- `KEYCLOAK_SERVER`, `KEYCLOAK_REALM`

## Architecture

### Workspace Structure (4 crates with dependency flow)

```
kernel → application → driver → server
```

- **kernel**: Domain entities, traits for Query/Modify/Event interfaces, Event Sourcing core
- **application**: Business logic services, cryptography (RSA-2048, Argon2id, AES-GCM), DTOs
- **driver**: PostgreSQL/Redis implementations of kernel interfaces
- **server**: Axum HTTP server, Keycloak auth, route handlers, event appliers

### Event Sourcing Pattern

Commands create events via `CommandEnvelope` → persisted to `event_streams` table → applied via `EventApplier` trait → projections updated in entity tables.

Key components:
- `EventApplier` trait (kernel/src/event.rs): Defines how events reconstruct entity state
- `Signal` trait (kernel/src/signal.rs): Triggers async event application via Redis message queue
- `ApplierContainer` (server/src/applier.rs): Manages entity-specific appliers using rikka-mq

### Interface Trait Pattern

Kernel defines interface traits, driver implements them:
- `DatabaseConnection` / `Transaction`: Database abstraction
- `*Query` traits: Read operations (e.g., `AccountQuery`)
- `*Modifier` traits: Write operations (e.g., `AccountModifier`)
- `DependOn*` traits: Dependency injection pattern

### Entity Structure

Entities use vodca macros (`References`, `Newln`, `Nameln`) and destructure for field access. Each entity has:
- ID type (UUID-based)
- Event enum with variants (Created, Updated, Deleted)
- `EventApplier` implementation

Domain entities: Account, AuthAccount, AuthHost, Profile, Metadata, Follow, Image, RemoteAccount

### Testing

Database tests use `#[test_with::env(DATABASE_URL)]` attribute to skip when database is unavailable.