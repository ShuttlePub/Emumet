use super::delivery::deliver_activity_to_inbox;
use super::ACTIVITYSTREAMS_CONTEXT;
use error_stack::Report;
use kernel::activitypub::Activity;
use kernel::interfaces::config::PublicBaseUrl;
use kernel::interfaces::crypto::{DependOnKeyEncryptor, DependOnPasswordProvider};
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::repository::DependOnSigningKeyRepository;
use kernel::prelude::entity::{AccountId, BlockId, RemoteAccount};
use kernel::KernelError;
use serde_json::Value;

pub(crate) fn block_activity(
    public_base_url: &PublicBaseUrl,
    block_id: &BlockId,
    local_actor_url: &str,
    remote_actor_url: &str,
) -> Activity {
    Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            block_id.as_ref()
        ),
        type_: "Block".to_string(),
        actor: local_actor_url.to_string(),
        object: Some(Value::String(remote_actor_url.to_string())),
        target: None,
        to: Some(vec![remote_actor_url.to_string()]),
        cc: None,
    }
}

pub(crate) fn undo_block_activity(
    public_base_url: &PublicBaseUrl,
    original_block: Activity,
    remote_actor_url: &str,
) -> error_stack::Result<Activity, KernelError> {
    let local_actor_url = original_block.actor.clone();
    let object = serde_json::to_value(original_block).map_err(|error| {
        Report::new(KernelError::Internal)
            .attach_printable(format!("Failed to serialize original Block: {error}"))
    })?;
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            kernel::generate_id()
        ),
        type_: "Undo".to_string(),
        actor: local_actor_url,
        object: Some(object),
        target: None,
        to: Some(vec![remote_actor_url.to_string()]),
        cc: None,
    })
}

pub(crate) async fn deliver_block_activity<T>(
    module: &T,
    account_id: &AccountId,
    activity: &Activity,
    remote_account: &RemoteAccount,
    activity_name: &str,
) -> error_stack::Result<(), KernelError>
where
    T: DependOnSigningKeyRepository
        + DependOnPasswordProvider
        + DependOnKeyEncryptor
        + DependOnHttpSigner
        + ?Sized,
{
    let inbox_url = remote_account.inbox_url().as_deref().ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Remote actor does not expose an inbox URL")
    })?;
    deliver_activity_to_inbox(module, account_id, inbox_url, activity, activity_name).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_activity_addresses_the_blocked_remote_actor() {
        kernel::ensure_generator_initialized();
        let block_id = BlockId::new(kernel::generate_id());

        let block = block_activity(
            &PublicBaseUrl::new("https://local.example".to_string()),
            &block_id,
            "https://local.example/ap/accounts/alice",
            "https://remote.example/users/bob",
        );

        assert_eq!(block.type_, "Block");
        assert_eq!(block.actor, "https://local.example/ap/accounts/alice");
        assert_eq!(
            block.object,
            Some(Value::String(
                "https://remote.example/users/bob".to_string()
            ))
        );
        assert_eq!(
            block.to,
            Some(vec!["https://remote.example/users/bob".to_string()])
        );
        assert!(block.id.ends_with(block_id.as_ref().to_string().as_str()));
    }

    #[test]
    fn undo_wraps_the_original_block_activity() {
        kernel::ensure_generator_initialized();
        let block_id = BlockId::new(kernel::generate_id());
        let original = block_activity(
            &PublicBaseUrl::new("https://local.example".to_string()),
            &block_id,
            "https://local.example/ap/accounts/alice",
            "https://remote.example/users/bob",
        );
        let original_id = original.id.clone();

        let undo = undo_block_activity(
            &PublicBaseUrl::new("https://local.example".to_string()),
            original,
            "https://remote.example/users/bob",
        )
        .unwrap();

        assert_eq!(undo.type_, "Undo");
        assert_eq!(undo.actor, "https://local.example/ap/accounts/alice");
        let object = undo.object.unwrap();
        assert_eq!(object["type"], "Block");
        assert_eq!(object["id"], Value::String(original_id));
        assert_eq!(object["actor"], "https://local.example/ap/accounts/alice");
        assert_eq!(object["object"], "https://remote.example/users/bob");
        assert_eq!(
            undo.to,
            Some(vec!["https://remote.example/users/bob".to_string()])
        );
    }
}
