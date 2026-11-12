use super::fetch::{client_for_url, validate_fetch_url};
use super::ACTIVITY_JSON;
use error_stack::Report;
use kernel::activitypub::Actor;
use kernel::KernelError;
use reqwest::header::{ACCEPT, USER_AGENT};

#[derive(Debug)]
pub(crate) struct ResolvedRemoteActor {
    inbox_url: Option<String>,
}

impl ResolvedRemoteActor {
    /// The actor's own inbox (not a shared inbox), if the actor document exposes one.
    pub(crate) fn inbox_url(&self) -> Option<&str> {
        self.inbox_url.as_deref()
    }
}

/// Test-only: global cache of resolved remote actor data.
///
/// `resolve_remote_actor` checks this before making an HTTP request.
/// Keys are inserted by the E2E test when the remote server requires
/// authentication for its ActivityPub actor endpoint.
#[cfg(any(test, feature = "test-mode"))]
use std::collections::HashMap;

#[cfg(any(test, feature = "test-mode"))]
use std::sync::{LazyLock, Mutex};

#[cfg(any(test, feature = "test-mode"))]
static TEST_STATIC_RESOLVED_ACTORS: LazyLock<Mutex<HashMap<String, ResolvedRemoteActor>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Insert a remote actor into the test global cache so that
/// `resolve_remote_actor` returns it without making an HTTP request.
#[cfg(any(test, feature = "test-mode"))]
pub fn inject_test_remote_actor(actor_url: &str, inbox_url: &str) {
    let mut cache = TEST_STATIC_RESOLVED_ACTORS.lock().expect("poisoned lock");
    cache.insert(
        actor_url.trim_end_matches('/').to_string(),
        ResolvedRemoteActor {
            inbox_url: Some(inbox_url.to_string()),
        },
    );
}

pub(super) async fn resolve_remote_actor(
    actor_url: &str,
) -> error_stack::Result<ResolvedRemoteActor, KernelError> {
    // Check test-mode global cache first
    #[cfg(any(test, feature = "test-mode"))]
    if let Ok(cache) = TEST_STATIC_RESOLVED_ACTORS.lock() {
        let key = actor_url.trim_end_matches('/');
        if let Some(cached) = cache.get(key) {
            return Ok(ResolvedRemoteActor {
                inbox_url: cached.inbox_url.clone(),
            });
        }
    }
    let mut url = reqwest::Url::parse(actor_url).map_err(|e| {
        Report::new(KernelError::Rejected).attach_printable(format!("Invalid actor URL: {e}"))
    })?;
    let mut redirects = 0;
    let actor = loop {
        if redirects > 5 {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Too many redirects while resolving remote actor"));
        }
        let resolved_addresses = validate_fetch_url(&url).await?;
        let response = client_for_url(&url, &resolved_addresses)?
            .get(url.clone())
            .header(ACCEPT, ACTIVITY_JSON)
            .header(USER_AGENT, "Emumet/0.1 ActivityPub actor resolver")
            .send()
            .await
            .map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("Remote actor fetch failed: {e}"))
            })?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    Report::new(KernelError::Rejected)
                        .attach_printable("Remote actor redirect without Location header")
                })?;
            url = url.join(location).map_err(|e| {
                Report::new(KernelError::Rejected)
                    .attach_printable(format!("Remote actor redirect URL is invalid: {e}"))
            })?;
            redirects += 1;
            continue;
        }
        if !response.status().is_success() {
            let status = response.status();
            let body_text = response.text().await.unwrap_or_default();
            tracing::debug!(
                remote_actor_url = %url,
                status = %status,
                body = %body_text,
                "Remote actor fetch failed with non-success status"
            );
            return Err(Report::new(KernelError::Rejected)
                .attach_printable(format!("Remote actor endpoint returned {status}",)));
        }
        break response.json::<Actor>().await.map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("Remote actor JSON is invalid: {e}"))
        })?;
    };

    let actor_id = reqwest::Url::parse(&actor.id).map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("Remote actor id is invalid: {e}"))
    })?;
    actor_id.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("Remote actor id has no host")
    })?;
    if actor.preferred_username.trim().is_empty() {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("Remote actor preferredUsername is empty"));
    }
    if !same_activitypub_id(&actor.id, actor_url) {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "Remote actor id does not match requested actor URL: expected {actor_url}, got {}",
            actor.id
        )));
    }

    Ok(ResolvedRemoteActor {
        inbox_url: Some(actor.inbox),
    })
}

fn same_activitypub_id(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}
