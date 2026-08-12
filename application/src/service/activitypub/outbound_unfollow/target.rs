use super::{resolve_remote_actor_identifier, upsert_remote_account};
use adapter::processor::account::AccountQueryProcessor;
use kernel::interfaces::config::PublicBaseUrl;
use kernel::interfaces::repository::RemoteAccountRepository;
use kernel::prelude::entity::{Account, AccountName, Nanoid, RemoteAccount};
use kernel::KernelError;

pub(super) enum UnfollowTarget {
    Local(Account),
    Remote(RemoteAccount),
}

pub(super) async fn resolve_unfollow_target<Q, R>(
    accounts: &Q,
    remote_accounts: &R,
    executor: &mut Q::Executor,
    public_base_url: &PublicBaseUrl,
    target: &str,
) -> error_stack::Result<UnfollowTarget, KernelError>
where
    Q: AccountQueryProcessor,
    R: RemoteAccountRepository<Executor = Q::Executor>,
{
    if let Some(account) = accounts
        .find_by_nanoid(executor, &Nanoid::<Account>::new(target.to_string()))
        .await?
    {
        return Ok(UnfollowTarget::Local(account));
    }
    if let Some(nanoid) = local_actor_nanoid(public_base_url, target) {
        if let Some(account) = accounts
            .find_by_nanoid(executor, &Nanoid::<Account>::new(nanoid))
            .await?
        {
            return Ok(UnfollowTarget::Local(account));
        }
    }
    if let Some(name) = local_acct_name(public_base_url, target) {
        if let Some(account) = accounts
            .find_by_name(executor, &AccountName::new(name))
            .await?
        {
            return Ok(UnfollowTarget::Local(account));
        }
    }

    let actor = resolve_remote_actor_identifier(target).await?;
    upsert_remote_account(remote_accounts, executor, actor)
        .await
        .map(UnfollowTarget::Remote)
}

fn local_actor_nanoid(public_base_url: &PublicBaseUrl, target: &str) -> Option<String> {
    let base = reqwest::Url::parse(public_base_url.as_str()).ok()?;
    let actor = reqwest::Url::parse(target).ok()?;
    if base.scheme() != actor.scheme()
        || base.host_str() != actor.host_str()
        || base.port_or_known_default() != actor.port_or_known_default()
    {
        return None;
    }
    actor
        .path()
        .strip_prefix("/ap/accounts/")
        .filter(|nanoid| !nanoid.is_empty() && !nanoid.contains('/'))
        .map(str::to_string)
}

fn local_acct_name(public_base_url: &PublicBaseUrl, target: &str) -> Option<String> {
    let identifier = target.strip_prefix("acct:").unwrap_or(target);
    let (name, domain) = identifier.split_once('@')?;
    let base = reqwest::Url::parse(public_base_url.as_str()).ok()?;
    let local_domain = match base.port() {
        Some(port) => format!("{}:{port}", base.host_str()?),
        None => base.host_str()?.to_string(),
    };
    (!name.is_empty() && domain.eq_ignore_ascii_case(&local_domain)).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::activitypub::ActorUrlBuilder;

    #[test]
    fn recognizes_local_actor_url() {
        let base = PublicBaseUrl::new("https://local.example".to_string());
        assert_eq!(
            local_actor_nanoid(&base, "https://local.example/ap/accounts/abc123"),
            Some("abc123".to_string())
        );
        assert_eq!(
            local_actor_nanoid(&base, "https://remote.example/ap/accounts/abc123"),
            None
        );
    }

    #[test]
    fn recognizes_local_acct_with_or_without_prefix() {
        let base = PublicBaseUrl::new("https://local.example:8443".to_string());
        assert_eq!(
            local_acct_name(&base, "acct:alice@local.example:8443"),
            Some("alice".to_string())
        );
        assert_eq!(
            local_acct_name(&base, "alice@local.example:8443"),
            Some("alice".to_string())
        );
        assert_eq!(local_acct_name(&base, "alice@remote.example"), None);
    }

    #[test]
    fn actor_builder_path_matches_local_detection() {
        let base = PublicBaseUrl::new("https://local.example".to_string());
        let actor = ActorUrlBuilder::new(base.as_str(), "abc123").actor_id();
        assert_eq!(
            local_actor_nanoid(&base, &actor),
            Some("abc123".to_string())
        );
    }
}
