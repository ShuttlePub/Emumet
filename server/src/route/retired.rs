//! Explicit "gone" answers for the Account based federation surface that the P4 cutover retired
//! (ADR profile-federation-in-place-reset §6).
//!
//! - `/ap/accounts/...` (actor, inbox, outbox, followers, following) answers `410 Gone`. It does
//!   NOT redirect to the new Profile actor: the old Account identity is deliberately not carried
//!   over (decision q14).
//! - `/internal/v1/accounts/{id}/sign` and `.../public-key` answer `410 Gone` even for a valid
//!   token (they sit behind the same authentication layer, so an unauthenticated caller still
//!   gets 401). There is no replacement signing API: a Profile's public key is read from its
//!   actor document and signing happens only inside the delivery worker.
//! - The Account scoped Follow / block / mute operations
//!   (`/api/v1/accounts/{account_id}/follow|unfollow|followers|following|block|unblock|blocks|mute|unmute|mutes`)
//!   answer `410 Gone`; their replacement is the Profile scoped API under
//!   `/api/v1/profiles/{profile_nanoid}/...`.
//!
//! The answers carry no state change and no secret, and they are part of the production root
//! router so that the "retired" behaviour is checked on the router the server really serves.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::Router;

const RETIRED_BODY_AP: &str = "{\"error\":\"gone\",\"message\":\"Account based ActivityPub actors were retired. Federation is per Profile: see /ap/profiles/{profile_nanoid}.\"}";
const RETIRED_BODY_SIGNING: &str = "{\"error\":\"gone\",\"message\":\"The Account signing and public-key endpoints were retired. A Profile public key is published in its actor document; there is no signing API.\"}";
const RETIRED_BODY_ACCOUNT_RELATIONS: &str = "{\"error\":\"gone\",\"message\":\"Account scoped follow, block and mute operations were retired. Use /api/v1/profiles/{profile_nanoid}/follows, /blocks and /mutes.\"}";

fn gone(body: &'static str) -> Response {
    (
        StatusCode::GONE,
        [(header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

async fn retired_ap() -> Response {
    gone(RETIRED_BODY_AP)
}

async fn retired_signing() -> Response {
    gone(RETIRED_BODY_SIGNING)
}

async fn retired_account_relations() -> Response {
    gone(RETIRED_BODY_ACCOUNT_RELATIONS)
}

pub trait RetiredRouter {
    /// Routes relative to `/ap`: every `/accounts` path.
    fn route_retired_account_federation(self) -> Self;
    /// Routes relative to `/internal/v1` (authenticated subtree).
    fn route_retired_signing(self) -> Self;
    /// Routes relative to `/api/v1` (authenticated subtree).
    fn route_retired_account_relations(self) -> Self;
}

impl<S> RetiredRouter for Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    fn route_retired_account_federation(self) -> Self {
        self.route("/accounts", any(retired_ap))
            .route("/accounts/{*rest}", any(retired_ap))
    }

    fn route_retired_signing(self) -> Self {
        self.route("/accounts/{id}/sign", any(retired_signing))
            .route("/accounts/{id}/public-key", any(retired_signing))
    }

    fn route_retired_account_relations(self) -> Self {
        let mut router = self;
        for suffix in [
            "follow",
            "unfollow",
            "followers",
            "following",
            "block",
            "unblock",
            "blocks",
            "mute",
            "unmute",
            "mutes",
        ] {
            router = router.route(
                &format!("/accounts/{{account_id}}/{suffix}"),
                any(retired_account_relations),
            );
        }
        router
    }
}
