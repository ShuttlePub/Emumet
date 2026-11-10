use crate::api::RegistrationEmailApi;
use crate::error::ErrorStatus;
use crate::schema::registration_protection::{
    ConfirmEmailChallengeRequest, CreateEmailChallengeRequest, EmailChallengeResponse,
    EmailProofResponse,
};
use application::service::registration_protection::{ConfirmOutcome, IssueOutcome};
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use std::net::SocketAddr;

mod source;

const INVALID_REQUEST: &str = "invalid request";
const UNAVAILABLE: &str = "registration email verification is currently unavailable";
const INVALID_CODE: &str = "the code is invalid or expired";

/// Kratos の flow 照会に転送する cookie。Kratos の CSRF cookie (`csrf_token_*`) だけを取り出し、
/// セッション等その他の cookie は Kratos へ渡さない。
fn kratos_csrf_cookie(headers: &HeaderMap) -> Option<String> {
    let selected: Vec<String> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|pair| {
            pair.starts_with("csrf_token_")
                && pair.contains('=')
                && pair.bytes().all(|b| b.is_ascii_graphic())
                && pair.len() <= 512
        })
        .map(str::to_string)
        .collect();
    (!selected.is_empty()).then(|| selected.join("; "))
}

#[utoipa::path(
    post,
    path = "/api/v1/registration/email-challenges",
    description = "Unauthenticated. Start a pre-registration email ownership challenge: a one-time code is sent to the address and bound to the normalized email, the real registration flow (flow_id) and the current configuration generation. Sending the mail is NOT proof of ownership; a proof is issued only after the code is confirmed (see `/confirm`). Abuse controls: the code is sent only while the registration gate accepts registrations and the email protection is operational (otherwise 503 and nothing is sent), per-email (1 per 60 s, 5 per hour), per-flow (3 per hour) and per-source (connection address, default 20 per hour) rate limits answer 429 with Retry-After. The response does not depend on whether the address exists, is registered or was rejected by the mail server (no user enumeration). The challenge expires after 10 minutes (never later than the flow) and allows 5 confirmation attempts. The flow is checked against Kratos (existence and its real expiry). A browser flow can only be checked with its Kratos CSRF cookie (`csrf_token_*`), so send this request with the browser credentials (cookies) for the Kratos domain; only those cookies are forwarded to Kratos. A flow whose expiry cannot be confirmed is rejected (400), never guessed.",
    request_body = CreateEmailChallengeRequest,
    responses(
        (status = 202, description = "Challenge created (a code is sent to the address)", body = EmailChallengeResponse),
        (status = 400, description = "Invalid email or flow"),
        (status = 429, description = "Rate limited (see Retry-After)"),
        (status = 503, description = "Email verification is unavailable; nothing was sent"),
    ),
    tag = "RegistrationProtection",
)]
pub(crate) async fn create_email_challenge(
    State(api): State<RegistrationEmailApi>,
    connect: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    request: Result<Json<CreateEmailChallengeRequest>, JsonRejection>,
) -> Result<Response, ErrorStatus> {
    let Ok(Json(request)) = request else {
        return Ok((StatusCode::BAD_REQUEST, INVALID_REQUEST).into_response());
    };
    let source = match source::from_request(
        connect.map(|Extension(ConnectInfo(addr))| addr.ip()),
        &headers,
    ) {
        Ok(source) => source,
        Err(source::SourceError::Configuration) => {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE).into_response());
        }
        Err(source::SourceError::ForwardedChain) => {
            return Ok((StatusCode::BAD_REQUEST, INVALID_REQUEST).into_response());
        }
    };
    let outcome = api
        .issue_challenge(
            &request.email,
            &request.flow_id,
            &source,
            kratos_csrf_cookie(&headers).as_deref(),
        )
        .await
        .map_err(ErrorStatus::from)?;
    Ok(match outcome {
        IssueOutcome::Issued {
            challenge_id,
            expires_at,
        } => (
            StatusCode::ACCEPTED,
            Json(EmailChallengeResponse {
                challenge_id,
                expires_at,
            }),
        )
            .into_response(),
        IssueOutcome::InvalidEmail | IssueOutcome::InvalidFlow => {
            (StatusCode::BAD_REQUEST, INVALID_REQUEST).into_response()
        }
        IssueOutcome::RateLimited { retry_after_secs } => (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, retry_after_secs.to_string())],
            "too many requests",
        )
            .into_response(),
        IssueOutcome::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, UNAVAILABLE).into_response(),
    })
}

#[utoipa::path(
    post,
    path = "/api/v1/registration/email-challenges/{challenge_id}/confirm",
    description = "Unauthenticated. Confirm the emailed code. Only a correct code issues an opaque `proof` (shown once), bound to the challenge's flow, email and configuration generation, valid for 15 minutes (never later than the flow). The proof is passed to registration as `transient_payload.email_proof` and verified and consumed atomically by the registration hook (E15d); it is never a permission on its own. A wrong code consumes one of 5 attempts (the challenge is invalidated afterwards). Unknown challenge, wrong code, expired, exhausted and old-generation challenges are indistinguishable (400).",
    params(("challenge_id" = String, Path, description = "Challenge ID returned by the create endpoint")),
    request_body = ConfirmEmailChallengeRequest,
    responses(
        (status = 200, description = "Code confirmed; proof issued", body = EmailProofResponse),
        (status = 400, description = "Invalid request, or the code is invalid or expired"),
    ),
    tag = "RegistrationProtection",
)]
pub(crate) async fn confirm_email_challenge(
    State(api): State<RegistrationEmailApi>,
    Path(challenge_id): Path<String>,
    request: Result<Json<ConfirmEmailChallengeRequest>, JsonRejection>,
) -> Result<Response, ErrorStatus> {
    let Ok(Json(request)) = request else {
        return Ok((StatusCode::BAD_REQUEST, INVALID_REQUEST).into_response());
    };
    let outcome = api
        .confirm_challenge(&challenge_id, &request.code)
        .await
        .map_err(ErrorStatus::from)?;
    Ok(match outcome {
        ConfirmOutcome::Confirmed { proof, expires_at } => {
            Json(EmailProofResponse { proof, expires_at }).into_response()
        }
        ConfirmOutcome::Rejected => (StatusCode::BAD_REQUEST, INVALID_CODE).into_response(),
    })
}

#[cfg(test)]
mod tests {
    use super::kratos_csrf_cookie;
    use axum::http::{header, HeaderMap, HeaderValue};

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(header::COOKIE, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    #[test]
    fn only_kratos_csrf_cookies_are_forwarded_to_kratos() {
        assert_eq!(kratos_csrf_cookie(&HeaderMap::new()), None);
        assert_eq!(
            kratos_csrf_cookie(&headers(&["session=secret; theme=dark"])),
            None
        );
        assert_eq!(
            kratos_csrf_cookie(&headers(&[
                "session=secret; csrf_token_abc123=tok; ory_kratos_session=s3cret",
                "csrf_token_def=other",
            ])),
            Some("csrf_token_abc123=tok; csrf_token_def=other".to_string())
        );
        // 値のない名前だけの cookie や名前が似ているだけの cookie は転送しない。
        assert_eq!(
            kratos_csrf_cookie(&headers(&["csrf_token_x; xcsrf_token_y=1"])),
            None
        );
    }
}
