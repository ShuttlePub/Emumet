use super::fetch::{client_for_url, validate_fetch_url};
use super::ACTIVITY_JSON;
use error_stack::Report;
use kernel::activitypub::Actor;
use kernel::interfaces::repository::RemoteAccountRepository;
use kernel::prelude::entity::{
    RemoteAccount, RemoteAccountAcct, RemoteAccountId, RemoteAccountUrl,
};
use kernel::KernelError;
use reqwest::header::{ACCEPT, USER_AGENT};

#[derive(Debug)]
pub(super) struct ResolvedRemoteActor {
    acct: RemoteAccountAcct,
    url: RemoteAccountUrl,
    inbox_url: Option<String>,
    public_key_pem: Option<String>,
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
pub fn inject_test_remote_actor(
    actor_url: &str,
    username: &str,
    inbox_url: &str,
    public_key_pem: &str,
) {
    let mut cache = TEST_STATIC_RESOLVED_ACTORS.lock().expect("poisoned lock");
    let actor_id_url = actor_url.trim_end_matches('/');
    let host = reqwest::Url::parse(actor_id_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| "local".to_string());
    cache.insert(
        actor_id_url.to_string(),
        ResolvedRemoteActor {
            acct: RemoteAccountAcct::new(format!("{}@{}", username, host)),
            url: RemoteAccountUrl::new(actor_id_url.to_string()),
            inbox_url: Some(inbox_url.to_string()),
            public_key_pem: Some(public_key_pem.to_string()),
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
                acct: RemoteAccountAcct::new(cached.acct.as_ref().clone()),
                url: RemoteAccountUrl::new(cached.url.as_ref().clone()),
                inbox_url: cached.inbox_url.clone(),
                public_key_pem: cached.public_key_pem.clone(),
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
    let host = actor_id.host_str().ok_or_else(|| {
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
        acct: RemoteAccountAcct::new(format!("{}@{}", actor.preferred_username, host)),
        url: RemoteAccountUrl::new(actor.id),
        inbox_url: Some(actor.inbox),
        public_key_pem: Some(actor.public_key.public_key_pem),
    })
}

pub(super) async fn resolve_remote_actor_identifier(
    identifier: &str,
) -> error_stack::Result<ResolvedRemoteActor, KernelError> {
    if let Some((user, domain)) = identifier
        .strip_prefix("acct:")
        .and_then(|s| s.split_once('@'))
    {
        if user.is_empty() || domain.is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Invalid acct: format, expected acct:user@domain"));
        }
        return resolve_remote_webfinger(user, domain).await;
    }
    resolve_remote_actor(identifier).await
}

async fn resolve_remote_webfinger(
    user: &str,
    domain: &str,
) -> error_stack::Result<ResolvedRemoteActor, KernelError> {
    if !user
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("Invalid characters in WebFinger user"));
    }
    if !domain
        .chars()
        .all(|c| c.is_alphanumeric() || c == '.' || c == '-')
        || domain.starts_with('-')
        || domain.ends_with('-')
    {
        return Err(Report::new(KernelError::Rejected)
            .attach_printable("Invalid characters in WebFinger domain"));
    }
    let webfinger_url = format!(
        "https://{}/.well-known/webfinger?resource=acct:{}@{}",
        domain, user, domain
    );
    let url = reqwest::Url::parse(&webfinger_url).map_err(|e| {
        Report::new(KernelError::Rejected).attach_printable(format!("Invalid WebFinger URL: {e}"))
    })?;
    let resolved_addresses = validate_fetch_url(&url).await?;
    let response = client_for_url(&url, &resolved_addresses)?
        .get(url)
        .header(ACCEPT, "application/jrd+json")
        .header(USER_AGENT, "Emumet/0.1 WebFinger resolver")
        .send()
        .await
        .map_err(|e| {
            Report::new(KernelError::Rejected)
                .attach_printable(format!("WebFinger request failed: {e}"))
        })?;
    if !response.status().is_success() {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "WebFinger returned {} for {}@{}",
            response.status(),
            user,
            domain
        )));
    }
    let jrd: serde_json::Value = response.json().await.map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("WebFinger response is not valid JSON: {e}"))
    })?;
    let subject = jrd.get("subject").and_then(|s| s.as_str()).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("WebFinger response missing subject field")
    })?;
    let expected_subject = format!("acct:{}@{}", user, domain);
    if subject != expected_subject {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "WebFinger subject mismatch: expected {expected_subject}, got {subject}"
        )));
    }
    let actor_url = jrd
        .get("links")
        .and_then(|links| links.as_array())
        .and_then(|links| {
            links.iter().find_map(|link| {
                let rel = link.get("rel").and_then(|r| r.as_str())?;
                let type_ = link.get("type").and_then(|t| t.as_str())?;
                let href = link.get("href").and_then(|h| h.as_str())?;
                if rel == "self"
                    && (type_ == "application/activity+json" || type_ == "application/ld+json")
                {
                    Some(href.to_string())
                } else {
                    None
                }
            })
        })
        .ok_or_else(|| {
            Report::new(KernelError::NotFound).attach_printable(format!(
                "No ActivityPub link found in WebFinger response for {}@{}",
                user, domain
            ))
        })?;
    resolve_remote_actor(&actor_url).await
}

pub(super) async fn upsert_remote_account<R, E>(
    repository: &R,
    executor: &mut E,
    actor: ResolvedRemoteActor,
) -> error_stack::Result<RemoteAccount, KernelError>
where
    R: RemoteAccountRepository<Executor = E>,
    E: kernel::interfaces::database::Executor,
{
    if let Some(existing) = repository.find_by_url(executor, &actor.url).await? {
        let updated = RemoteAccount::new(
            existing.id().clone(),
            actor.acct,
            actor.url,
            existing.icon_id().clone(),
            actor.inbox_url,
            actor.public_key_pem,
        );
        repository.update(executor, &updated).await?;
        return Ok(updated);
    }

    let remote_account = RemoteAccount::new(
        RemoteAccountId::new(kernel::generate_id()),
        actor.acct,
        actor.url,
        None,
        actor.inbox_url,
        actor.public_key_pem,
    );
    repository.create(executor, &remote_account).await?;
    Ok(remote_account)
}

fn same_activitypub_id(left: &str, right: &str) -> bool {
    left.trim_end_matches('/') == right.trim_end_matches('/')
}
