use super::*;
use driver::database::PostgresDatabase;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
use kernel::interfaces::repository::{
    DependOnFollowRepository, DependOnOutboxActivityRepository, FollowRepository,
    OutboxActivityRepository,
};
use kernel::prelude::entity::{AccountId, FollowApprovedAt, FollowId, RemoteAccountId};
use kernel::test_utils::{unique_account_name, AccountBuilder, FollowBuilder};

#[test]
fn undo_wraps_the_original_follow_activity() {
    kernel::ensure_generator_initialized();
    let follow = Follow::new(
        FollowId::new(kernel::generate_id()),
        FollowTargetId::from(AccountId::default()),
        FollowTargetId::from(RemoteAccountId::new(kernel::generate_id())),
        None,
    )
    .unwrap();

    let undo = undo_follow_activity(
        &kernel::interfaces::config::PublicBaseUrl::new("https://local.example".to_string()),
        &follow,
        "https://local.example/ap/accounts/alice",
        "https://remote.example/users/bob",
    )
    .unwrap();

    assert_eq!(undo.type_, "Undo");
    let original = undo.object.unwrap();
    assert_eq!(original["type"], "Follow");
    assert_eq!(original["actor"], "https://local.example/ap/accounts/alice");
    assert_eq!(original["object"], "https://remote.example/users/bob");
    assert!(original["id"]
        .as_str()
        .is_some_and(|id| id.ends_with(follow.id().as_ref().to_string().as_str())));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn local_unfollow_deletes_approved_follow_without_outbox_activity() {
    kernel::ensure_generator_initialized();
    let database = PostgresDatabase::new().await.unwrap();
    let mut executor = database.connection().await.unwrap();
    let source = AccountBuilder::new()
        .id(AccountId::default())
        .name(unique_account_name())
        .build();
    let destination = AccountBuilder::new()
        .id(AccountId::default())
        .name(unique_account_name())
        .build();
    database
        .account_read_model()
        .create(&mut executor, &source)
        .await
        .unwrap();
    database
        .account_read_model()
        .create(&mut executor, &destination)
        .await
        .unwrap();
    let follow = FollowBuilder::new()
        .source_local(source.id().clone())
        .destination_local(destination.id().clone())
        .approved_at(Some(FollowApprovedAt::default()))
        .build();
    database
        .follow_repository()
        .create(&mut executor, &follow)
        .await
        .unwrap();
    let outbox_before = database
        .outbox_activity_repository()
        .count_by_account_id(&mut executor, source.id())
        .await
        .unwrap();

    delete_approved_follow(
        database.follow_repository(),
        &mut executor,
        &FollowTargetId::from(source.id().clone()),
        &FollowTargetId::from(destination.id().clone()),
    )
    .await
    .unwrap();

    let remaining = database
        .follow_repository()
        .find_followings(&mut executor, &FollowTargetId::from(source.id().clone()))
        .await
        .unwrap();
    let outbox_after = database
        .outbox_activity_repository()
        .count_by_account_id(&mut executor, source.id())
        .await
        .unwrap();
    assert!(remaining.is_empty());
    assert_eq!(outbox_after, outbox_before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn local_unfollow_rejects_pending_or_missing_follow() {
    kernel::ensure_generator_initialized();
    let database = PostgresDatabase::new().await.unwrap();
    let mut executor = database.connection().await.unwrap();
    let source = AccountBuilder::new()
        .id(AccountId::default())
        .name(unique_account_name())
        .build();
    let destination = AccountBuilder::new()
        .id(AccountId::default())
        .name(unique_account_name())
        .build();
    database
        .account_read_model()
        .create(&mut executor, &source)
        .await
        .unwrap();
    database
        .account_read_model()
        .create(&mut executor, &destination)
        .await
        .unwrap();
    let pending = FollowBuilder::new()
        .source_local(source.id().clone())
        .destination_local(destination.id().clone())
        .build();
    database
        .follow_repository()
        .create(&mut executor, &pending)
        .await
        .unwrap();
    let source_id = FollowTargetId::from(source.id().clone());
    let destination_id = FollowTargetId::from(destination.id().clone());

    let pending_error = delete_approved_follow(
        database.follow_repository(),
        &mut executor,
        &source_id,
        &destination_id,
    )
    .await
    .unwrap_err();
    assert_eq!(pending_error.current_context(), &KernelError::NotFound);
    database
        .follow_repository()
        .delete(&mut executor, pending.id())
        .await
        .unwrap();
    let missing_error = delete_approved_follow(
        database.follow_repository(),
        &mut executor,
        &source_id,
        &destination_id,
    )
    .await
    .unwrap_err();
    assert_eq!(missing_error.current_context(), &KernelError::NotFound);
}
