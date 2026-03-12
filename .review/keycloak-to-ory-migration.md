---
feature: keycloak-to-ory-migration
started: 2026-03-11
phase: implementing
---

# Keycloak to Ory Migration (Phase 1: Kratos + Hydra)

## Requirements

### Purpose

Replace Keycloak with Ory Kratos (identity management) + Ory Hydra (OAuth2/OIDC) to reduce infrastructure weight. Enable self-service registration. Login/Consent UI is ShuttlePub frontend's responsibility.

### Scope - Do

- Remove all Keycloak dependencies from server layer (`axum-keycloak-auth`, `KeycloakAuthLayer`, `KeycloakToken`, `expect_role!` macro)
- Implement JWT validation middleware for Hydra-issued tokens
  - Validate: `iss`, `aud`, `exp`, `sub`
  - JWKS: auto-resolve via OIDC Discovery, in-memory cache, re-fetch on kid miss (rate-limited), lazy init on startup failure
  - Audience: configurable via `EXPECTED_AUDIENCE` env var
- Implement Login/Consent Provider backend in server layer (Hydra login/consent accept/reject API)
- Value mapping: `AuthHost.url` = Hydra issuer URL, `AuthAccount.client_id` = Kratos identity UUID (JWT `sub`)
- Simplify authorization to auth-check only (JWT validity). Role-based authz deferred to Phase 2 (Ketos)
- Dev environment: Keycloak -> Kratos + Hydra containers, self-service registration enabled
- Kratos identity schema: minimal (email + password)
- Seed data: test user (email: testuser@example.com / password: testuser) via Kratos import
- Existing AuthHost/AuthAccount data: discard (dev only, no migration)
- Env vars: `HYDRA_ISSUER_URL`, `HYDRA_ADMIN_URL`, `EXPECTED_AUDIENCE`
- Update server-layer auth tests. JWT unit tests use test RSA keypair + mock JWKS. Integration tests use `test_with::env` pattern

### Scope - Don't

- Ory Ketos introduction (Phase 2)
- Authorization checker trait in kernel (Phase 2)
- Multi-AuthAccount permission separation (Phase 2+)
- Entity structure changes (AuthAccount, AuthHost, etc.)
- kernel / adapter / application / driver layer changes
- Login UI implementation (ShuttlePub frontend responsibility)

## Design

### A. JWT Validation Middleware

Replace `axum-keycloak-auth` with `jsonwebtoken` + `reqwest`.

```
server/src/auth.rs (new, replaces keycloak.rs)
├── OidcConfig        — issuer URL, expected audience, jwks_refetch_interval_secs (configurable for tests)
├── JwksCache         — in-memory (Arc<RwLock<JwkSet>>), kid miss → re-fetch (rate-limited), lazy init
├── AuthClaims        — standard OIDC claims (iss, sub, aud, exp)
├── OidcAuthInfo      — from AuthClaims: issuer → AuthHost.url, subject → AuthAccount.client_id
├── auth_middleware()  — axum middleware: Bearer token → validate → Extension<AuthClaims>
└── resolve_auth_account_id()  — rewritten with OidcAuthInfo (same find-or-create logic)
```

Value mapping: JWT `iss` → AuthHost.url, JWT `sub` (= Kratos identity UUID) → AuthAccount.client_id.
Hydra login accept sets `subject = Kratos identity.id`, so JWT `sub` = Kratos identity UUID.

### B. Login/Consent Provider

```
server/src/route/oauth2.rs (new) — NOT under auth_middleware
├── GET  /oauth2/login    — login_challenge → Kratos session check → Hydra login accept/redirect
├── GET  /oauth2/consent  — consent_challenge → skip check → unified JSON response
│                           skip:     { action: "redirect", redirect_to: "..." }
│                           non-skip: { action: "show_consent", consent_challenge, client_name, requested_scope }
├── POST /oauth2/consent  — consent result → Hydra consent accept/reject
```

### C. Hydra Admin API Client

```
server/src/hydra.rs (new, reqwest-based)
├── HydraAdminClient
│   ├── get_login_request / accept_login / reject_login
│   └── get_consent_request / accept_consent / reject_consent
```

### D. Kratos Client

```
server/src/kratos.rs (new)
├── KratosClient
│   └── whoami(cookie) -> Option<KratosSession>
```

Domain premise: Kratos and Emumet on same domain (subdomain) for cookie reachability.

### E. Route Handler Changes

account.rs, profile.rs, metadata.rs: KeycloakToken → AuthClaims, remove KeycloakAuthLayer, remove expect_role!.
route.rs: remove `to_permission_strings` and its test, add oauth2 module.

Router structure in main.rs:
```
Router::new()
    .route_account/profile/metadata(...)
    .layer(auth_middleware(...))      // JWT required
    .route_oauth2(...)               // NO auth_middleware
    .layer(CorsLayer)
    .with_state(app)
```

### F. Handler / AppModule

Handler gets: HydraAdminClient, KratosClient, JwksCache, OidcConfig as fields.
Access via `#[derive(References)]` auto-generated getters. No DependOn* traits needed (server-layer only).

### G. Dev Environment

podman-compose (docker-compose.yml). Helm charts for production separately later.

```
ory/
├── kratos/
│   ├── kratos.yml, identity.schema.json, seed-users.json
└── hydra/
    └── hydra.yml
docker-compose.yml  — postgres, redis, kratos, hydra
```

Env vars: HYDRA_ISSUER_URL, HYDRA_ADMIN_URL, KRATOS_PUBLIC_URL, EXPECTED_AUDIENCE

### H. Data Cleanup

Dev switch: TRUNCATE auth_hosts, auth_accounts, auth_account_events. No schema migration needed.

### I. Files NOT Changed/Deleted

- permission.rs: kept (potential Phase 2 reference)
- kernel / adapter / application / driver: no changes

### J. Testing

- JWT validation: test RSA keypair + in-memory JwkSet injection (configurable refetch interval = 0)
- Login/Consent: test_with::env(HYDRA_ADMIN_URL) skip
- Route handlers: Extension<AuthClaims> directly set, bypass middleware

## Tasks

- [x] 1. Dev environment (Ory Kratos + Hydra)
  - [x] 1.1 Kratos config files (kratos.yml, identity.schema.json, seed-users.json) (P)
  - [x] 1.2 Hydra config file (hydra.yml) (P)
  - [x] 1.3 docker-compose.yml (postgres, redis, kratos, hydra; replaces standalone podman run; shared postgres with DB name separation)
  - [x] 1.4 .env.example update (add HYDRA_ISSUER_URL, HYDRA_ADMIN_URL, KRATOS_PUBLIC_URL, EXPECTED_AUDIENCE; remove KEYCLOAK_SERVER, KEYCLOAK_REALM)
  - [x] 1.5 Startup verification (podman-compose up, health endpoints respond)
- [x] 2. Cargo.toml deps and type definitions
  - [x] 2.1 Add jsonwebtoken / reqwest to Cargo.toml (keep axum-keycloak-auth for now)
  - [x] 2.2 OidcConfig / AuthClaims / OidcAuthInfo type definitions (server/src/auth.rs new)
- [x] 3. JWT validation and external clients
  - [x] 3.1 JwksCache (OIDC Discovery, in-memory cache, kid miss re-fetch, lazy init) (P)
  - [x] 3.2 HydraAdminClient types and methods (server/src/hydra.rs) (P)
  - [x] 3.3 KratosClient and whoami (server/src/kratos.rs) (P)
  - [x] 3.4 auth_middleware (Bearer extraction, JWT validation, Extension<AuthClaims>)
  - [x] 3.5 JWT validation unit tests (test RSA keypair + JwkSet injection)
- [x] R1. Code review: auth foundation (auth.rs, hydra.rs, kratos.rs)
- [x] 4. Login/Consent Provider endpoints
  - [x] 4.1 OAuth2Router trait and oauth2 module in route.rs
  - [x] 4.2 GET /oauth2/login endpoint
  - [x] 4.3 GET /oauth2/consent endpoint (skip check, unified JSON response)
  - [x] 4.4 POST /oauth2/consent endpoint (consent result -> Hydra accept/reject)
- [x] R2. Code review: OAuth2 flow (route/oauth2.rs)
- [x] 5. Keycloak removal and route handler rewrite
  - [x] 5.1 Rewrite resolve_auth_account_id with OidcAuthInfo (in auth.rs)
  - [x] 5.2 account.rs rewrite (KeycloakToken -> AuthClaims, remove KeycloakAuthLayer/expect_role!) (P)
  - [x] 5.3 profile.rs rewrite (same) (P)
  - [x] 5.4 metadata.rs rewrite (same) (P)
  - [x] 5.5 Remove to_permission_strings + test from route.rs, remove expect_role! macro from keycloak.rs
  - [x] 5.6 Route handler unit test updates (Extension<AuthClaims> direct injection)
- [x] 6. Handler / AppModule / main.rs integration
  - [x] 6.1 Handler: add HydraAdminClient / KratosClient / JwksCache / OidcConfig fields + AppModule accessors + init
  - [x] 6.2 main.rs rewrite (remove KeycloakAuthInstance, add auth_middleware + OAuth2 routes, middleware scoping)
  - [x] 6.3 Remove axum-keycloak-auth from Cargo.toml
- [x] R3. Code review: integration (handler.rs, main.rs, route handlers consistency)
- [x] 7. Cleanup and documentation
  - [x] 7.1 Delete keycloak.rs, keycloak-data/, remove keycloak-data lines from .gitignore (P)
  - [x] 7.2 Update CLAUDE.md / README.md (podman-compose setup instructions) (P)
  - [x] 7.3 Document data cleanup procedure (TRUNCATE auth_hosts / auth_accounts / auth_account_events)
- [x] 8. Integration tests and verification
  - [ ] 8.1 Login/Consent flow integration test (test_with::env(HYDRA_ADMIN_URL) skip)
  - [x] 8.2 E2E verification with podman-compose (register -> login -> JWT -> API access)
- [x] R4. Final review: full alignment check (requirements/design coverage, missing items, code quality)
