use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{DependOnFollowRepository, FollowRepository};
use kernel::prelude::entity::{
    AccountId, Follow, FollowApprovedAt, FollowId, FollowTargetId, RemoteAccountId,
};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(sqlx::FromRow)]
struct FollowRow {
    id: i64,
    follower_local_id: Option<i64>,
    follower_remote_id: Option<i64>,
    followee_local_id: Option<i64>,
    followee_remote_id: Option<i64>,
    approved_at: Option<OffsetDateTime>,
}

impl TryFrom<FollowRow> for Follow {
    type Error = Report<KernelError>;

    fn try_from(value: FollowRow) -> Result<Self, Self::Error> {
        let id = FollowId::new(value.id);
        let source = match (value.follower_local_id, value.follower_remote_id) {
            (Some(follower_local_id), None) => {
                FollowTargetId::from(AccountId::new(follower_local_id))
            }
            (None, Some(follower_remote_id)) => {
                FollowTargetId::from(RemoteAccountId::new(follower_remote_id))
            }
            _ => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Invalid follow data. follower_local_id: {:?}, follower_remote_id: {:?}",
                    value.follower_local_id, value.follower_remote_id
                )))
            }
        };
        let destination = match (value.followee_local_id, value.followee_remote_id) {
            (Some(followee_local_id), None) => {
                FollowTargetId::from(AccountId::new(followee_local_id))
            }
            (None, Some(followee_remote_id)) => {
                FollowTargetId::from(RemoteAccountId::new(followee_remote_id))
            }
            _ => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Invalid follow data. followee_local_id: {:?}, followee_remote_id: {:?}",
                    value.followee_local_id, value.followee_remote_id
                )))
            }
        };
        let approved_at = value.approved_at.map(FollowApprovedAt::new);

        Follow::new(id, source, destination, approved_at)
    }
}

pub struct PostgresFollowRepository;

fn split_follow_target_id(target_id: &FollowTargetId) -> (Option<&i64>, Option<&i64>) {
    match target_id {
        FollowTargetId::Local(account_id) => (Some(account_id.as_ref()), None),
        FollowTargetId::Remote(remote_account_id) => (None, Some(remote_account_id.as_ref())),
    }
}

impl FollowRepository for PostgresFollowRepository {
    type Executor = PostgresConnection;

    async fn find_followings(
        &self,
        executor: &mut Self::Executor,
        source_id: &FollowTargetId,
    ) -> error_stack::Result<Vec<Follow>, KernelError> {
        let con: &mut PgConnection = executor;
        match source_id {
            FollowTargetId::Local(account_id) => {
                sqlx::query_as::<_, FollowRow>(
                    //language=postgresql
                    r#"
            SELECT id, follower_local_id, follower_remote_id, followee_local_id, followee_remote_id, approved_at
            FROM follows
            WHERE follower_local_id = $1
            "#
                ).bind(account_id.as_ref())
            }
            FollowTargetId::Remote(remote_account_id) => {
                sqlx::query_as::<_, FollowRow>(
                    //language=postgresql
                    r#"
            SELECT id, follower_local_id, follower_remote_id, followee_local_id, followee_remote_id, approved_at
            FROM follows
            WHERE follower_remote_id = $1
            "#
                ).bind(remote_account_id.as_ref())
            }
        }.fetch_all(con)
            .await
            .convert_error()
            .and_then(|rows| rows.into_iter().map(Follow::try_from).collect::<Result<_, _>>())
    }

    async fn find_followers(
        &self,
        executor: &mut Self::Executor,
        destination_id: &FollowTargetId,
    ) -> error_stack::Result<Vec<Follow>, KernelError> {
        let con: &mut PgConnection = executor;
        match destination_id {
            FollowTargetId::Local(account_id) => {
                sqlx::query_as::<_, FollowRow>(
                    //language=postgresql
                    r#"
            SELECT id, follower_local_id, follower_remote_id, followee_local_id, followee_remote_id, approved_at
            FROM follows
            WHERE followee_local_id = $1 AND approved_at IS NOT NULL
            "#
                ).bind(account_id.as_ref())
            }
            FollowTargetId::Remote(remote_account_id) => {
                sqlx::query_as::<_, FollowRow>(
                    //language=postgresql
                    r#"
            SELECT id, follower_local_id, follower_remote_id, followee_local_id, followee_remote_id, approved_at
            FROM follows
            WHERE followee_remote_id = $1 AND approved_at IS NOT NULL
            "#
                ).bind(remote_account_id.as_ref())
            }
        }.fetch_all(con)
            .await
            .convert_error()
            .and_then(|rows| rows.into_iter().map(Follow::try_from).collect::<Result<_, _>>())
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        follow: &Follow,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (follower_local_id, follower_remote_id) = split_follow_target_id(follow.source());
        let (followee_local_id, followee_remote_id) = split_follow_target_id(follow.destination());
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO follows (id, follower_local_id, follower_remote_id, followee_local_id, followee_remote_id, approved_at)
            VALUES ($1, $2, $3, $4, $5, $6)
            "#
        ).bind(follow.id().as_ref())
            .bind(follower_local_id)
            .bind(follower_remote_id)
            .bind(followee_local_id)
            .bind(followee_remote_id)
            .bind(follow.approved_at().as_ref().map(FollowApprovedAt::as_ref))
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Executor,
        follow: &Follow,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (follower_local_id, follower_remote_id) = split_follow_target_id(follow.source());
        let (followee_local_id, followee_remote_id) = split_follow_target_id(follow.destination());
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE follows
            SET follower_local_id = $2, follower_remote_id = $3, followee_local_id = $4, followee_remote_id = $5, approved_at = $6
            WHERE id = $1
            "#
        ).bind(follow.id().as_ref())
            .bind(follower_local_id)
            .bind(follower_remote_id)
            .bind(followee_local_id)
            .bind(followee_remote_id)
            .bind(follow.approved_at().as_ref().map(FollowApprovedAt::as_ref))
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        follow_id: &FollowId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM follows WHERE id = $1
            "#,
        )
        .bind(follow_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }
}

impl DependOnFollowRepository for PostgresDatabase {
    type FollowRepository = PostgresFollowRepository;

    fn follow_repository(&self) -> &Self::FollowRepository {
        &PostgresFollowRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
        use kernel::interfaces::repository::{DependOnFollowRepository, FollowRepository};
        use kernel::prelude::entity::{AccountId, FollowApprovedAt, FollowTargetId};
        use kernel::test_utils::{AccountBuilder, FollowBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_followers() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();
            let follower_id = AccountId::default();
            let follower_account = AccountBuilder::new()
                .id(follower_id.clone())
                .name("follower")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &follower_account)
                .await
                .unwrap();
            let followee_id = AccountId::default();
            let followee_account = AccountBuilder::new()
                .id(followee_id.clone())
                .name("followee")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &followee_account)
                .await
                .unwrap();
            let follow = FollowBuilder::new()
                .source_local(follower_id.clone())
                .destination_local(followee_id.clone())
                .build();

            database
                .follow_repository()
                .create(&mut transaction, &follow)
                .await
                .unwrap();

            let followers = database
                .follow_repository()
                .find_followings(&mut transaction, &FollowTargetId::from(follower_id))
                .await
                .unwrap();
            assert_eq!(followers[0].id(), follow.id());

            let followers = database
                .follow_repository()
                .find_followings(&mut transaction, &FollowTargetId::from(followee_id))
                .await
                .unwrap();
            assert!(followers.is_empty());
            database
                .follow_repository()
                .delete(&mut transaction, follow.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, follower_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, followee_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_followings() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();
            let follower_id = AccountId::default();
            let follower_account = AccountBuilder::new()
                .id(follower_id.clone())
                .name("follower")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &follower_account)
                .await
                .unwrap();
            let followee_id = AccountId::default();
            let followee_account = AccountBuilder::new()
                .id(followee_id.clone())
                .name("followee")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &followee_account)
                .await
                .unwrap();
            let follow = FollowBuilder::new()
                .source_local(follower_id.clone())
                .destination_local(followee_id.clone())
                .approved_at(Some(FollowApprovedAt::default()))
                .build();

            database
                .follow_repository()
                .create(&mut transaction, &follow)
                .await
                .unwrap();

            let followings = database
                .follow_repository()
                .find_followers(&mut transaction, &FollowTargetId::from(followee_id))
                .await
                .unwrap();
            assert_eq!(followings[0].id(), follow.id());

            let followings = database
                .follow_repository()
                .find_followers(&mut transaction, &FollowTargetId::from(follower_id))
                .await
                .unwrap();
            assert!(followings.is_empty());
            database
                .follow_repository()
                .delete(&mut transaction, follow.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, follower_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, followee_account.id())
                .await
                .unwrap();
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
        use kernel::interfaces::repository::{DependOnFollowRepository, FollowRepository};
        use kernel::prelude::entity::{AccountId, Follow, FollowApprovedAt, FollowTargetId};
        use kernel::test_utils::{AccountBuilder, FollowBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();
            let follower_id = AccountId::default();
            let follower_account = AccountBuilder::new()
                .id(follower_id.clone())
                .name("follower")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &follower_account)
                .await
                .unwrap();
            let followee_id = AccountId::default();
            let followee_account = AccountBuilder::new()
                .id(followee_id.clone())
                .name("followee")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &followee_account)
                .await
                .unwrap();
            let follow = FollowBuilder::new()
                .source_local(follower_id)
                .destination_local(followee_id)
                .approved_at(Some(FollowApprovedAt::default()))
                .build();

            database
                .follow_repository()
                .create(&mut transaction, &follow)
                .await
                .unwrap();
            database
                .follow_repository()
                .delete(&mut transaction, follow.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, follower_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, followee_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let follower_id = AccountId::default();
            let follower_account = AccountBuilder::new()
                .id(follower_id.clone())
                .name("follower")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &follower_account)
                .await
                .unwrap();
            let followee_id = AccountId::default();
            let followee_account = AccountBuilder::new()
                .id(followee_id.clone())
                .name("followee")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &followee_account)
                .await
                .unwrap();
            let follow = FollowBuilder::new()
                .source_local(follower_id.clone())
                .destination_local(followee_id.clone())
                .build();

            let following = database
                .follow_repository()
                .find_followings(&mut transaction, &FollowTargetId::from(follower_id.clone()))
                .await
                .unwrap();
            assert!(following.is_empty());

            database
                .follow_repository()
                .create(&mut transaction, &follow)
                .await
                .unwrap();

            let follow = Follow::new(
                follow.id().clone(),
                FollowTargetId::from(follower_id.clone()),
                FollowTargetId::from(followee_id.clone()),
                Some(FollowApprovedAt::default()),
            )
            .unwrap();
            database
                .follow_repository()
                .update(&mut transaction, &follow)
                .await
                .unwrap();

            let following = database
                .follow_repository()
                .find_followers(&mut transaction, &FollowTargetId::from(followee_id))
                .await
                .unwrap();
            assert_eq!(following[0].id(), follow.id());
            database
                .follow_repository()
                .delete(&mut transaction, follow.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, follower_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, followee_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();
            let follower_id = AccountId::default();
            let follower_account = AccountBuilder::new()
                .id(follower_id.clone())
                .name("follower")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &follower_account)
                .await
                .unwrap();
            let followee_id = AccountId::default();
            let followee_account = AccountBuilder::new()
                .id(followee_id.clone())
                .name("followee")
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &followee_account)
                .await
                .unwrap();
            let follow = FollowBuilder::new()
                .source_local(follower_id.clone())
                .destination_local(followee_id)
                .build();

            database
                .follow_repository()
                .create(&mut transaction, &follow)
                .await
                .unwrap();

            database
                .follow_repository()
                .delete(&mut transaction, follow.id())
                .await
                .unwrap();

            let following = database
                .follow_repository()
                .find_followers(&mut transaction, &FollowTargetId::from(follower_id))
                .await
                .unwrap();
            assert!(following.is_empty());
            database
                .account_read_model()
                .deactivate(&mut transaction, follower_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, followee_account.id())
                .await
                .unwrap();
        }
    }
}
