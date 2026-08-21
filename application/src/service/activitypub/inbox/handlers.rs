use super::super::remote_actor::{resolve_remote_actor, upsert_remote_account};
use super::super::{local_actor_url, ACTIVITYSTREAMS_CONTEXT};
use super::InboxUseCase;
use crate::dto::activitypub::InboxActivityDto;
use crate::service::block::{block_target_to_follow_target, remove_follows_between};
use error_stack::Report;
use kernel::activitypub::Activity;
use kernel::interfaces::config::PublicBaseUrl;
use kernel::interfaces::database::{DatabaseConnection, TransactionManager};
use kernel::interfaces::repository::{
    BlockRepository, FollowRepository, OutboxActivityRepository, RemoteAccountRepository,
};
use kernel::prelude::entity::{
    Block, BlockId, BlockTargetId, Follow, FollowApprovedAt, FollowId, FollowTargetId,
    OutboxActivity, RemoteAccountUrl,
};
use kernel::KernelError;
use serde_json::Value;

pub(super) async fn handle_follow_activity<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase,
{
    let followed_actor_url = activity_object_id(&dto.activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Follow activity object must be an actor id")
    })?;
    ensure_local_actor_matches(
        module.public_base_url(),
        &dto.account_nanoid,
        &followed_actor_url,
    )?;

    let remote_actor = resolve_remote_actor(&dto.activity.actor).await?;
    let mut executor = module.database_connection().connection().await?;
    let remote_account = upsert_remote_account(
        module.remote_account_repository(),
        &mut executor,
        remote_actor,
    )
    .await?;

    let source = FollowTargetId::from(remote_account.id().clone());
    let destination = FollowTargetId::from(dto.account_id.clone());
    let follow = Follow::new(
        FollowId::new(kernel::generate_id()),
        source,
        destination,
        Some(FollowApprovedAt::default()),
    )?;

    let deps = module.clone();
    let account_id = dto.account_id.clone();
    let account_id_for_delivery = account_id.clone();
    let account_nanoid = dto.account_nanoid.clone();
    let inbox_url = remote_account.inbox_url().clone();
    let original_follow = dto.activity.clone();
    let (accepted, delivery) = module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                let inserted = deps
                    .follow_repository()
                    .insert_if_absent(executor, &follow)
                    .await?;

                if !inserted {
                    tracing::debug!("Follow already exists, skipping Accept creation");
                    return Ok((false, None));
                }

                let local_actor_url = local_actor_url(deps.public_base_url(), &account_nanoid);
                let accept = accept_activity(
                    deps.public_base_url(),
                    &follow,
                    &local_actor_url,
                    original_follow,
                )?;

                let outbox_entry = OutboxActivity {
                    id: 0,
                    account_id: account_id.clone(),
                    activity_id: accept.id.clone(),
                    activity_type: "Accept".to_string(),
                    object_json: serde_json::to_string(&accept).map_err(|error| {
                        Report::new(KernelError::Internal).attach_printable(format!(
                            "Failed to serialize Accept activity to JSON: {error}"
                        ))
                    })?,
                    created_at: time::OffsetDateTime::now_utc(),
                    delivered_at: None,
                    attempted_at: None,
                    error: None,
                };
                let outbox_id = deps
                    .outbox_activity_repository()
                    .create(executor, &outbox_entry)
                    .await?;

                Ok((true, Some((outbox_id, accept))))
            })
        })
        .await?;

    if let (true, Some((outbox_id, accept))) = (accepted, delivery) {
        match &inbox_url {
            Some(inbox_url) => {
                if let Err(error) = module
                    .deliver_accept(&account_id_for_delivery, &outbox_id, inbox_url, &accept)
                    .await
                {
                    tracing::warn!(?error, inbox_url, "Failed to deliver ActivityPub Accept");
                }
            }
            None => {
                tracing::warn!(
                    "Remote actor does not expose an inbox URL; Accept stays pending in the outbox"
                );
            }
        }
    }

    Ok(())
}

pub(super) async fn handle_undo_follow<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase,
{
    let follow_activity = undo_follow_object(&dto.activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Undo activity object must be a Follow activity")
    })?;
    let followed_actor_url = activity_object_id(&follow_activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Undo Follow object must target an actor id")
    })?;
    ensure_local_actor_matches(
        module.public_base_url(),
        &dto.account_nanoid,
        &followed_actor_url,
    )?;

    let mut executor = module.database_connection().connection().await?;
    let remote_url = RemoteAccountUrl::new(dto.activity.actor.clone());
    let Some(remote_account) = module
        .remote_account_repository()
        .find_by_url(&mut executor, &remote_url)
        .await?
    else {
        return Ok(());
    };

    let source = FollowTargetId::from(remote_account.id().clone());
    let destination = FollowTargetId::from(dto.account_id);
    module
        .follow_repository()
        .delete_if_exists(&mut executor, &source, &destination)
        .await?;
    Ok(())
}

pub(super) async fn handle_accept_activity<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase,
{
    let accept = &dto.activity;
    let nested_follow = accept
        .object
        .as_ref()
        .and_then(|obj| serde_json::from_value::<Activity>(obj.clone()).ok())
        .filter(|a| a.type_ == "Follow")
        .ok_or_else(|| {
            Report::new(KernelError::Rejected)
                .attach_printable("Accept object must be a Follow activity")
        })?;

    let follow_actor_url = nested_follow.actor.trim_end_matches('/').to_string();
    let expected_local = local_actor_url(module.public_base_url(), &dto.account_nanoid);
    if follow_actor_url != expected_local.trim_end_matches('/') {
        tracing::debug!(
            follow_actor = %nested_follow.actor,
            expected = %expected_local,
            "Accept Follow actor does not match local actor"
        );
        return Ok(());
    }

    let remote_actor_url = activity_object_id(&nested_follow).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Accept Follow object must have an actor id")
    })?;

    let accept_actor = accept.actor.trim_end_matches('/').to_string();
    if accept_actor != remote_actor_url.trim_end_matches('/') {
        tracing::debug!(
            accept_actor = %accept.actor,
            remote_actor = %remote_actor_url,
            "Accept actor does not match Follow object"
        );
        return Ok(());
    }

    let mut executor = module.database_connection().connection().await?;
    let remote_url = RemoteAccountUrl::new(remote_actor_url.clone());
    let remote_account = module
        .remote_account_repository()
        .find_by_url(&mut executor, &remote_url)
        .await?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("Remote account not found for {remote_actor_url}"))
        })?;

    let source = FollowTargetId::from(dto.account_id.clone());
    let destination = FollowTargetId::from(remote_account.id().clone());
    let approved = module
        .follow_repository()
        .approve_follow_if_pending(&mut executor, &source, &destination)
        .await?;

    if approved {
        tracing::info!(
            remote_actor = %remote_actor_url,
            "Follow approved via Accept activity"
        );
    } else {
        tracing::debug!(
            remote_actor = %remote_actor_url,
            "No pending follow found for Accept activity"
        );
    }
    Ok(())
}

pub(super) async fn handle_block_activity<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase,
{
    let blocked_actor_url = activity_object_id(&dto.activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Block activity object must be an actor id")
    })?;
    ensure_local_actor_matches(
        module.public_base_url(),
        &dto.account_nanoid,
        &blocked_actor_url,
    )?;

    let remote_actor = resolve_remote_actor(&dto.activity.actor).await?;
    let mut executor = module.database_connection().connection().await?;
    let remote_account = upsert_remote_account(
        module.remote_account_repository(),
        &mut executor,
        remote_actor,
    )
    .await?;

    let source = BlockTargetId::from(remote_account.id().clone());
    let destination = BlockTargetId::from(dto.account_id.clone());
    let block = Block::new(
        BlockId::new(kernel::generate_id()),
        source.clone(),
        destination.clone(),
    )?;

    let deps = module.clone();
    let account_id = dto.account_id;
    module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                let inserted = deps
                    .block_repository()
                    .insert_if_absent(executor, &block)
                    .await?;

                // A duplicate Block aborts the Postgres transaction at the
                // failed INSERT, so no further statement may run; the first
                // Block already removed the follows, so skipping is a no-op.
                if inserted {
                    remove_follows_between(
                        deps.follow_repository(),
                        executor,
                        &block_target_to_follow_target(&source),
                        &block_target_to_follow_target(&destination),
                    )
                    .await?;
                }
                Ok(())
            })
        })
        .await?;

    tracing::debug!(account_id = ?account_id, "Processed inbound Block");
    Ok(())
}

pub(super) async fn handle_undo_block_activity<T>(
    module: &T,
    dto: InboxActivityDto,
) -> error_stack::Result<(), KernelError>
where
    T: InboxUseCase,
{
    let block_activity = undo_block_object(&dto.activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Undo activity object must be a Block activity")
    })?;
    let blocked_actor_url = activity_object_id(&block_activity).ok_or_else(|| {
        Report::new(KernelError::Rejected)
            .attach_printable("Undo Block object must target an actor id")
    })?;
    ensure_local_actor_matches(
        module.public_base_url(),
        &dto.account_nanoid,
        &blocked_actor_url,
    )?;

    let mut executor = module.database_connection().connection().await?;
    let remote_url = RemoteAccountUrl::new(dto.activity.actor.clone());
    let Some(remote_account) = module
        .remote_account_repository()
        .find_by_url(&mut executor, &remote_url)
        .await?
    else {
        return Ok(());
    };

    let source = BlockTargetId::from(remote_account.id().clone());
    let destination = BlockTargetId::from(dto.account_id);
    let deps = module.clone();
    module
        .transaction_manager()
        .transaction(move |executor| {
            Box::pin(async move {
                deps.block_repository()
                    .delete_if_exists(executor, &source, &destination)
                    .await?;
                Ok(())
            })
        })
        .await?;
    Ok(())
}

pub(super) fn undo_object_is_block(activity: &Activity) -> bool {
    undo_block_object(activity).is_some()
}

fn undo_block_object(activity: &Activity) -> Option<Activity> {
    let object = activity.object.as_ref()?;
    serde_json::from_value::<Activity>(object.clone())
        .ok()
        .filter(|activity| activity.type_ == "Block")
}

pub(super) fn undo_object_is_follow(activity: &Activity) -> bool {
    undo_follow_object(activity).is_some()
}

fn undo_follow_object(activity: &Activity) -> Option<Activity> {
    let object = activity.object.as_ref()?;
    serde_json::from_value::<Activity>(object.clone())
        .ok()
        .filter(|activity| activity.type_ == "Follow")
}

fn activity_object_id(activity: &Activity) -> Option<String> {
    match activity.object.as_ref()? {
        Value::String(value) => Some(value.clone()),
        Value::Object(map) => map.get("id").and_then(Value::as_str).map(str::to_string),
        _ => None,
    }
}

fn ensure_local_actor_matches(
    public_base_url: &PublicBaseUrl,
    account_nanoid: &str,
    object_id: &str,
) -> error_stack::Result<(), KernelError> {
    let expected = local_actor_url(public_base_url, account_nanoid);
    if object_id.trim_end_matches('/') == expected {
        Ok(())
    } else {
        Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "Follow object does not match local actor: expected {expected}, got {object_id}"
        )))
    }
}

fn accept_activity(
    public_base_url: &PublicBaseUrl,
    follow: &Follow,
    actor: &str,
    original_follow: Activity,
) -> error_stack::Result<Activity, KernelError> {
    // The Accept activity must be directed TO the follower (original Follow's actor),
    // not to the local actor who is sending the Accept.
    let remote_follower_url = &original_follow.actor;
    let object = serde_json::to_value(original_follow.clone()).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable("Failed to serialize original Follow activity")
    })?;
    Ok(Activity {
        context: Some(Value::String(ACTIVITYSTREAMS_CONTEXT.to_string())),
        id: format!(
            "{}/activities/{}",
            public_base_url.as_str().trim_end_matches('/'),
            follow.id().as_ref()
        ),
        type_: "Accept".to_string(),
        actor: actor.to_string(),
        object: Some(object),
        target: None,
        to: Some(vec![remote_follower_url.to_string()]),
        cc: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::config::DependOnPublicBaseUrl;
    use kernel::interfaces::crypto::{
        DependOnKeyEncryptor, DependOnPasswordProvider, EncryptedPrivateKey, KeyEncryptor,
        PasswordProvider, SigningAlgorithm,
    };
    use kernel::interfaces::database::{
        Connection, DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager,
    };
    use kernel::interfaces::http_signing::{
        DependOnHttpSigner, HttpSigner, HttpSigningRequest, HttpSigningResponse,
    };
    use kernel::interfaces::repository::{
        DependOnBlockRepository, DependOnFollowRepository, DependOnOutboxActivityRepository,
        DependOnRemoteAccountRepository, DependOnSigningKeyRepository, SigningKeyRepository,
    };
    use kernel::prelude::entity::{
        AccountId, OutboxActivityId, RemoteAccount, RemoteAccountAcct, RemoteAccountId, SigningKey,
        SigningKeyId,
    };
    use std::pin::Pin;
    use zeroize::Zeroizing;

    #[derive(Clone)]
    struct MockConnection;

    impl Connection for MockConnection {}

    #[derive(Clone)]
    struct MockDatabaseConnection;

    impl DatabaseConnection for MockDatabaseConnection {
        type Connection = MockConnection;

        async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
            Ok(MockConnection)
        }
    }

    impl TransactionManager for MockDatabaseConnection {
        fn transaction<'a, F, T>(
            &'a self,
            operation: F,
        ) -> Pin<
            Box<dyn std::future::Future<Output = error_stack::Result<T, KernelError>> + Send + 'a>,
        >
        where
            F: for<'connection> FnOnce(
                    &'connection mut Self::Connection,
                ) -> Pin<
                    Box<
                        dyn std::future::Future<Output = error_stack::Result<T, KernelError>>
                            + Send
                            + 'connection,
                    >,
                > + Send
                + 'a,
            T: Send + 'a,
        {
            Box::pin(async move {
                let mut connection = MockConnection;
                operation(&mut connection).await
            })
        }
    }

    #[derive(Clone)]
    struct MockFollowRepository;

    impl FollowRepository for MockFollowRepository {
        type Connection = MockConnection;

        async fn find_followings(
            &self,
            _executor: &mut Self::Connection,
            _source: &FollowTargetId,
        ) -> error_stack::Result<Vec<Follow>, KernelError> {
            Ok(Vec::new())
        }

        async fn find_followers(
            &self,
            _executor: &mut Self::Connection,
            _destination: &FollowTargetId,
        ) -> error_stack::Result<Vec<Follow>, KernelError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _executor: &mut Self::Connection,
            _follow: &Follow,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn update(
            &self,
            _executor: &mut Self::Connection,
            _follow: &Follow,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn delete(
            &self,
            _executor: &mut Self::Connection,
            _follow_id: &FollowId,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn insert_if_absent(
            &self,
            _executor: &mut Self::Connection,
            _follow: &Follow,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(false)
        }

        async fn approve_follow_if_pending(
            &self,
            _executor: &mut Self::Connection,
            _source: &FollowTargetId,
            _destination: &FollowTargetId,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(false)
        }

        async fn delete_if_exists(
            &self,
            _executor: &mut Self::Connection,
            _source: &FollowTargetId,
            _destination: &FollowTargetId,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(false)
        }
    }

    #[derive(Clone)]
    struct MockBlockRepository;

    impl BlockRepository for MockBlockRepository {
        type Connection = MockConnection;

        async fn find_blocks(
            &self,
            _executor: &mut Self::Connection,
            _source: &BlockTargetId,
        ) -> error_stack::Result<Vec<Block>, KernelError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _executor: &mut Self::Connection,
            _block: &Block,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn delete(
            &self,
            _executor: &mut Self::Connection,
            _block_id: &BlockId,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn insert_if_absent(
            &self,
            _executor: &mut Self::Connection,
            _block: &Block,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(false)
        }

        async fn delete_if_exists(
            &self,
            _executor: &mut Self::Connection,
            _source: &BlockTargetId,
            _destination: &BlockTargetId,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(false)
        }
    }

    #[derive(Clone)]
    struct MockRemoteAccountRepository;

    impl RemoteAccountRepository for MockRemoteAccountRepository {
        type Connection = MockConnection;

        async fn find_by_id(
            &self,
            _executor: &mut Self::Connection,
            _id: &RemoteAccountId,
        ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
            Ok(None)
        }

        async fn find_by_acct(
            &self,
            _executor: &mut Self::Connection,
            _acct: &RemoteAccountAcct,
        ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
            Ok(None)
        }

        async fn find_by_url(
            &self,
            _executor: &mut Self::Connection,
            _url: &RemoteAccountUrl,
        ) -> error_stack::Result<Option<RemoteAccount>, KernelError> {
            Ok(None)
        }

        async fn create(
            &self,
            _executor: &mut Self::Connection,
            _account: &RemoteAccount,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn update(
            &self,
            _executor: &mut Self::Connection,
            _account: &RemoteAccount,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn delete(
            &self,
            _executor: &mut Self::Connection,
            _account_id: &RemoteAccountId,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct MockSigningKeyRepository;

    impl SigningKeyRepository for MockSigningKeyRepository {
        type Connection = MockConnection;

        async fn find_by_id(
            &self,
            _executor: &mut Self::Connection,
            _id: &SigningKeyId,
        ) -> error_stack::Result<SigningKey, KernelError> {
            Err(Report::new(KernelError::Internal).attach_printable("unused mock"))
        }

        async fn find_by_account_id(
            &self,
            _executor: &mut Self::Connection,
            _account_id: &AccountId,
        ) -> error_stack::Result<Vec<SigningKey>, KernelError> {
            Ok(Vec::new())
        }

        async fn find_active_by_account_id(
            &self,
            _executor: &mut Self::Connection,
            _account_id: &AccountId,
        ) -> error_stack::Result<Vec<SigningKey>, KernelError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _executor: &mut Self::Connection,
            _signing_key: &SigningKey,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn revoke(
            &self,
            _executor: &mut Self::Connection,
            _id: &SigningKeyId,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct MockOutboxActivityRepository;

    impl OutboxActivityRepository for MockOutboxActivityRepository {
        type Connection = MockConnection;

        async fn create(
            &self,
            _executor: &mut Self::Connection,
            _activity: &OutboxActivity,
        ) -> error_stack::Result<OutboxActivityId, KernelError> {
            Ok(0)
        }

        async fn find_by_account_id(
            &self,
            _executor: &mut Self::Connection,
            _account_id: &AccountId,
            _limit: usize,
            _cursor: Option<i64>,
        ) -> error_stack::Result<Vec<OutboxActivity>, KernelError> {
            Ok(Vec::new())
        }

        async fn count_by_account_id(
            &self,
            _executor: &mut Self::Connection,
            _account_id: &AccountId,
        ) -> error_stack::Result<u64, KernelError> {
            Ok(0)
        }

        async fn find_pending_deliveries(
            &self,
            _executor: &mut Self::Connection,
            _limit: usize,
        ) -> error_stack::Result<Vec<OutboxActivity>, KernelError> {
            Ok(Vec::new())
        }

        async fn mark_delivered(
            &self,
            _executor: &mut Self::Connection,
            _id: &OutboxActivityId,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn mark_delivery_attempt(
            &self,
            _executor: &mut Self::Connection,
            _id: &OutboxActivityId,
            _error: Option<&str>,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }
    }

    #[derive(Clone)]
    struct MockPasswordProvider;

    impl PasswordProvider for MockPasswordProvider {
        fn get_password(&self) -> error_stack::Result<Zeroizing<Vec<u8>>, KernelError> {
            Ok(Zeroizing::new(b"unused".to_vec()))
        }
    }

    #[derive(Clone)]
    struct MockKeyEncryptor;

    impl KeyEncryptor for MockKeyEncryptor {
        fn encrypt(
            &self,
            _private_key_pem: &[u8],
            _password: &[u8],
            _algorithm: SigningAlgorithm,
        ) -> error_stack::Result<EncryptedPrivateKey, KernelError> {
            Err(Report::new(KernelError::Internal).attach_printable("unused mock"))
        }

        fn decrypt(
            &self,
            _encrypted: &EncryptedPrivateKey,
            _password: &[u8],
        ) -> error_stack::Result<Zeroizing<Vec<u8>>, KernelError> {
            Err(Report::new(KernelError::Internal).attach_printable("unused mock"))
        }
    }

    #[derive(Clone)]
    struct MockHttpSigner;

    impl HttpSigner for MockHttpSigner {
        async fn sign(
            &self,
            _request: &HttpSigningRequest,
            _private_key_pem: &[u8],
            _key_id: &str,
            _algorithm: &SigningAlgorithm,
        ) -> error_stack::Result<HttpSigningResponse, KernelError> {
            Err(Report::new(KernelError::Internal).attach_printable("unused mock"))
        }
    }

    #[derive(Clone)]
    struct MockModule {
        database: MockDatabaseConnection,
        follows: MockFollowRepository,
        blocks: MockBlockRepository,
        remote_accounts: MockRemoteAccountRepository,
        signing_keys: MockSigningKeyRepository,
        outbox: MockOutboxActivityRepository,
        password_provider: MockPasswordProvider,
        key_encryptor: MockKeyEncryptor,
        http_signer: MockHttpSigner,
        public_base_url: PublicBaseUrl,
    }

    impl DependOnDatabaseConnection for MockModule {
        type DatabaseConnection = MockDatabaseConnection;

        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.database
        }
    }

    impl DependOnTransactionManager for MockModule {
        type TransactionManager = MockDatabaseConnection;

        fn transaction_manager(&self) -> &Self::TransactionManager {
            &self.database
        }
    }

    impl DependOnFollowRepository for MockModule {
        type FollowRepository = MockFollowRepository;

        fn follow_repository(&self) -> &Self::FollowRepository {
            &self.follows
        }
    }

    impl DependOnBlockRepository for MockModule {
        type BlockRepository = MockBlockRepository;

        fn block_repository(&self) -> &Self::BlockRepository {
            &self.blocks
        }
    }

    impl DependOnRemoteAccountRepository for MockModule {
        type RemoteAccountRepository = MockRemoteAccountRepository;

        fn remote_account_repository(&self) -> &Self::RemoteAccountRepository {
            &self.remote_accounts
        }
    }

    impl DependOnSigningKeyRepository for MockModule {
        type SigningKeyRepository = MockSigningKeyRepository;

        fn signing_key_repository(&self) -> &Self::SigningKeyRepository {
            &self.signing_keys
        }
    }

    impl DependOnOutboxActivityRepository for MockModule {
        type OutboxActivityRepository = MockOutboxActivityRepository;

        fn outbox_activity_repository(&self) -> &Self::OutboxActivityRepository {
            &self.outbox
        }
    }

    impl DependOnPasswordProvider for MockModule {
        type PasswordProvider = MockPasswordProvider;

        fn password_provider(&self) -> &Self::PasswordProvider {
            &self.password_provider
        }
    }

    impl DependOnKeyEncryptor for MockModule {
        type KeyEncryptor = MockKeyEncryptor;

        fn key_encryptor(&self) -> &Self::KeyEncryptor {
            &self.key_encryptor
        }
    }

    impl DependOnHttpSigner for MockModule {
        type HttpSigner = MockHttpSigner;

        fn http_signer(&self) -> &Self::HttpSigner {
            &self.http_signer
        }
    }

    impl DependOnPublicBaseUrl for MockModule {
        fn public_base_url(&self) -> &PublicBaseUrl {
            &self.public_base_url
        }
    }

    fn follow(source: AccountId, destination: AccountId, approved: bool) -> Follow {
        kernel::ensure_generator_initialized();
        Follow::new(
            FollowId::new(kernel::generate_id()),
            FollowTargetId::from(source),
            FollowTargetId::from(destination),
            approved.then(FollowApprovedAt::default),
        )
        .unwrap()
    }

    fn module() -> (MockModule, AccountId) {
        kernel::ensure_generator_initialized();
        let account_id = AccountId::default();

        (
            MockModule {
                database: MockDatabaseConnection,
                follows: MockFollowRepository,
                blocks: MockBlockRepository,
                remote_accounts: MockRemoteAccountRepository,
                signing_keys: MockSigningKeyRepository,
                outbox: MockOutboxActivityRepository,
                password_provider: MockPasswordProvider,
                key_encryptor: MockKeyEncryptor,
                http_signer: MockHttpSigner,
                public_base_url: PublicBaseUrl::new("https://example.com/".to_string()),
            },
            account_id,
        )
    }

    fn follow_activity(actor: &str, object: &str) -> Activity {
        Activity {
            context: None,
            id: "https://remote.example/activities/follow-1".to_string(),
            type_: "Follow".to_string(),
            actor: actor.to_string(),
            object: Some(serde_json::Value::String(object.to_string())),
            target: None,
            to: None,
            cc: None,
        }
    }

    fn block_activity(object: serde_json::Value) -> Activity {
        Activity {
            context: None,
            id: "https://remote.example/activities/block-1".to_string(),
            type_: "Block".to_string(),
            actor: "https://remote.example/users/bob".to_string(),
            object: Some(object),
            target: None,
            to: None,
            cc: None,
        }
    }

    fn undo_activity(object: Activity) -> Activity {
        Activity {
            context: None,
            id: "https://remote.example/activities/undo-1".to_string(),
            type_: "Undo".to_string(),
            actor: "https://remote.example/users/bob".to_string(),
            object: Some(serde_json::to_value(object).unwrap()),
            target: None,
            to: None,
            cc: None,
        }
    }

    fn inbox_dto(account_id: AccountId, activity: Activity) -> InboxActivityDto {
        InboxActivityDto {
            account_id,
            account_nanoid: "alice".to_string(),
            activity,
        }
    }

    #[test]
    fn undo_object_is_block_detects_nested_block() {
        let block = Activity {
            context: None,
            id: "https://remote.example/activities/block-1".to_string(),
            type_: "Block".to_string(),
            actor: "https://remote.example/users/bob".to_string(),
            object: Some(serde_json::Value::String(
                "https://example.com/accounts/alice".to_string(),
            )),
            target: None,
            to: None,
            cc: None,
        };
        let undo = Activity {
            context: None,
            id: "https://remote.example/activities/undo-block-1".to_string(),
            type_: "Undo".to_string(),
            actor: "https://remote.example/users/bob".to_string(),
            object: Some(serde_json::to_value(block).unwrap()),
            target: None,
            to: None,
            cc: None,
        };

        assert!(undo_object_is_block(&undo));
        assert!(!undo_object_is_follow(&undo));
    }

    #[test]
    fn undo_object_is_follow_detects_nested_follow() {
        let follow = follow_activity(
            "https://remote.example/users/bob",
            "https://example.com/accounts/alice",
        );
        let undo = Activity {
            context: None,
            id: "https://remote.example/activities/undo-1".to_string(),
            type_: "Undo".to_string(),
            actor: "https://remote.example/users/bob".to_string(),
            object: Some(serde_json::to_value(follow).unwrap()),
            target: None,
            to: None,
            cc: None,
        };

        assert!(undo_object_is_follow(&undo));
    }

    #[test]
    fn accept_activity_wraps_original_follow() {
        let (module, account_id) = module();
        let follow = follow(AccountId::default(), account_id, true);
        let original = follow_activity(
            "https://remote.example/users/bob",
            "https://example.com/accounts/alice",
        );

        let accept = accept_activity(
            module.public_base_url(),
            &follow,
            "https://example.com/accounts/alice",
            original,
        )
        .unwrap();

        assert_eq!(accept.type_, "Accept");
        assert_eq!(accept.actor, "https://example.com/accounts/alice");
        assert_eq!(
            accept.id,
            format!("https://example.com/activities/{}", follow.id().as_ref())
        );
        assert_eq!(
            accept.object.as_ref().and_then(|value| value.get("type")),
            Some(&serde_json::Value::String("Follow".to_string()))
        );
        // Accept must be directed TO the follower (original Follow's actor),
        // not to the local actor who is sending the Accept.
        assert_eq!(
            accept.to,
            Some(vec!["https://remote.example/users/bob".to_string()]),
            "Accept.to should target the remote follower, not the local actor"
        );
    }

    #[test]
    fn local_actor_match_rejects_wrong_follow_object() {
        let public_base_url = PublicBaseUrl::new("https://example.com/".to_string());

        assert!(ensure_local_actor_matches(
            &public_base_url,
            "alice",
            "https://example.com/ap/accounts/alice/"
        )
        .is_ok());
        assert!(ensure_local_actor_matches(
            &public_base_url,
            "alice",
            "https://example.com/accounts/bob"
        )
        .is_err());
    }

    #[tokio::test]
    async fn block_for_other_local_actor_is_rejected() {
        let (module, account_id) = module();
        let activity = block_activity(serde_json::Value::String(
            "https://example.com/ap/accounts/bob".to_string(),
        ));

        let error = module
            .handle_block_activity(inbox_dto(account_id, activity))
            .await
            .unwrap_err();

        assert!(matches!(error.current_context(), KernelError::Rejected));
    }

    #[tokio::test]
    async fn block_with_non_actor_object_is_rejected() {
        let (module, account_id) = module();
        let activity = block_activity(serde_json::json!({"type": "Note"}));

        let error = module
            .handle_block_activity(inbox_dto(account_id, activity))
            .await
            .unwrap_err();

        assert!(matches!(error.current_context(), KernelError::Rejected));
        assert!(format!("{error:?}").contains("Block activity object must be an actor id"));
    }

    #[tokio::test]
    async fn undo_block_wrapping_follow_is_rejected() {
        let (module, account_id) = module();
        let follow = follow_activity(
            "https://remote.example/users/bob",
            "https://example.com/ap/accounts/alice",
        );

        let error = module
            .handle_undo_block_activity(inbox_dto(account_id, undo_activity(follow)))
            .await
            .unwrap_err();

        assert!(matches!(error.current_context(), KernelError::Rejected));
        assert!(format!("{error:?}").contains("Undo activity object must be a Block activity"));
    }

    #[tokio::test]
    async fn undo_block_from_unknown_remote_actor_is_ok() {
        let (module, account_id) = module();
        let block = block_activity(serde_json::Value::String(
            "https://example.com/ap/accounts/alice".to_string(),
        ));

        let result = module
            .handle_undo_block_activity(inbox_dto(account_id, undo_activity(block)))
            .await;

        assert!(result.is_ok());
    }
}
