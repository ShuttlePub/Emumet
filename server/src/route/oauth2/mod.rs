mod consent;
mod login;

pub(crate) use consent::{__path_get_consent, __path_post_consent, get_consent, post_consent};
pub(crate) use login::{__path_login, login};

use crate::handler::AppModule;
use axum::routing::{get, post};
use axum::Router;

pub(super) const REMEMBER_FOR_SECS: i64 = 3600;

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub trait OAuth2Router {
    fn route_oauth2(self) -> Self;
}

impl OAuth2Router for Router<AppModule> {
    fn route_oauth2(self) -> Self {
        self.route("/oauth2/login", get(login))
            .route("/oauth2/consent", get(get_consent))
            .route("/oauth2/consent", post(post_consent))
    }
}

#[cfg(test)]
pub(super) mod test_support {
    use super::OAuth2Router;
    use crate::handler::AppModule;
    use axum::body::Body;
    use axum::http::StatusCode;
    use axum::Router;
    use http_body_util::BodyExt;

    pub(super) async fn build_app(hydra_url: &str, kratos_url: &str) -> Router {
        let app = AppModule::new_for_oauth2_test(hydra_url.into(), kratos_url.into())
            .await
            .unwrap();
        Router::new().route_oauth2().with_state(app)
    }

    pub(super) fn assert_redirect(resp: &axum::http::Response<Body>, expected_url: &str) {
        assert_eq!(resp.status(), StatusCode::FOUND);
        let location = resp
            .headers()
            .get(axum::http::header::LOCATION)
            .expect("missing Location header")
            .to_str()
            .unwrap();
        assert_eq!(location, expected_url);
    }

    pub(super) async fn response_json(resp: axum::http::Response<Body>) -> serde_json::Value {
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&body).unwrap_or_else(|e| {
            panic!(
                "Failed to parse response as JSON: {e}\nBody: {}",
                String::from_utf8_lossy(&body)
            )
        })
    }
}
