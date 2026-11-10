mod discovery;
mod inbox_support;
pub(crate) mod profile_actor;
pub(crate) mod profile_inbox;

pub(crate) use profile_actor::{__path_get_profile_actor, __path_profile_webfinger};
pub(crate) use profile_inbox::__path_post_profile_inbox;

use crate::error::ErrorStatus;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

pub(super) const ACTIVITY_JSON: &str = "application/activity+json";
pub(super) const JRD_JSON: &str = "application/jrd+json";

pub(super) fn json_response<T: Serialize>(
    value: &T,
    content_type: &'static str,
) -> Result<Response, ErrorStatus> {
    let body = serde_json::to_vec(value).map_err(|e| {
        ErrorStatus::from((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to serialize JSON response: {e}"),
        ))
    })?;
    Ok(([(header::CONTENT_TYPE, content_type)], body).into_response())
}
