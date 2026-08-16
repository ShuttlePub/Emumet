use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{BlockRepository, DependOnBlockRepository};
use kernel::prelude::entity::{AccountId, Block, BlockId, BlockTargetId, RemoteAccountId};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct BlockRow {
    id: i64,
    blocker_local_id: Option<i64>,
    blocker_remote_id: Option<i64>,
    blocked_local_id: Option<i64>,
    blocked_remote_id: Option<i64>,
}

impl TryFrom<BlockRow> for Block {
    type Error = Report<KernelError>;

    fn try_from(value: BlockRow) -> Result<Self, Self::Error> {
        let id = BlockId::new(value.id);
        let source = match (value.blocker_local_id, value.blocker_remote_id) {
            (Some(blocker_local_id), None) => BlockTargetId::from(AccountId::new(blocker_local_id)),
            (None, Some(blocker_remote_id)) => {
                BlockTargetId::from(RemoteAccountId::new(blocker_remote_id))
            }
            _ => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Invalid block data. blocker_local_id: {:?}, blocker_remote_id: {:?}",
                    value.blocker_local_id, value.blocker_remote_id
                )))
            }
        };
        let destination = match (value.blocked_local_id, value.blocked_remote_id) {
            (Some(blocked_local_id), None) => BlockTargetId::from(AccountId::new(blocked_local_id)),
            (None, Some(blocked_remote_id)) => {
                BlockTargetId::from(RemoteAccountId::new(blocked_remote_id))
            }
            _ => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Invalid block data. blocked_local_id: {:?}, blocked_remote_id: {:?}",
                    value.blocked_local_id, value.blocked_remote_id
                )))
            }
        };

        Block::new(id, source, destination)
    }
}

pub struct PostgresBlockRepository;

fn split_block_target_id(target_id: &BlockTargetId) -> (Option<&i64>, Option<&i64>) {
    match target_id {
        BlockTargetId::Local(account_id) => (Some(account_id.as_ref()), None),
        BlockTargetId::Remote(remote_account_id) => (None, Some(remote_account_id.as_ref())),
    }
}

impl BlockRepository for PostgresBlockRepository {
    type Connection = PostgresConnection;

    async fn find_blocks(
        &self,
        executor: &mut Self::Connection,
        source_id: &BlockTargetId,
    ) -> error_stack::Result<Vec<Block>, KernelError> {
        let con: &mut PgConnection = executor;
        match source_id {
            BlockTargetId::Local(account_id) => {
                sqlx::query_as::<_, BlockRow>(
                    //language=postgresql
                    r#"
            SELECT id, blocker_local_id, blocker_remote_id, blocked_local_id, blocked_remote_id
            FROM blocks
            WHERE blocker_local_id = $1
            "#,
                )
                .bind(account_id.as_ref())
            }
            BlockTargetId::Remote(remote_account_id) => {
                sqlx::query_as::<_, BlockRow>(
                    //language=postgresql
                    r#"
            SELECT id, blocker_local_id, blocker_remote_id, blocked_local_id, blocked_remote_id
            FROM blocks
            WHERE blocker_remote_id = $1
            "#,
                )
                .bind(remote_account_id.as_ref())
            }
        }
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|rows| {
            rows.into_iter()
                .map(Block::try_from)
                .collect::<Result<_, _>>()
        })
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
        block: &Block,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (blocker_local_id, blocker_remote_id) = split_block_target_id(block.source());
        let (blocked_local_id, blocked_remote_id) = split_block_target_id(block.destination());
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO blocks (id, blocker_local_id, blocker_remote_id, blocked_local_id, blocked_remote_id)
            VALUES ($1, $2, $3, $4, $5)
            "#
        ).bind(block.id().as_ref())
            .bind(blocker_local_id)
            .bind(blocker_remote_id)
            .bind(blocked_local_id)
            .bind(blocked_remote_id)
            .execute(con)
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err))
                if db_err.code().is_some_and(|code| code == "23505") =>
            {
                Err(Report::new(KernelError::Rejected)
                    .attach_printable("Duplicate block: this block relationship already exists"))
            }
            Err(e) => Err(Report::from(e).change_context(KernelError::Internal)),
        }
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        block_id: &BlockId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM blocks WHERE id = $1
            "#,
        )
        .bind(block_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target block not found for delete"));
        }
        Ok(())
    }

    async fn insert_if_absent(
        &self,
        executor: &mut Self::Connection,
        block: &Block,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let (blocker_local_id, blocker_remote_id) = split_block_target_id(block.source());
        let (blocked_local_id, blocked_remote_id) = split_block_target_id(block.destination());
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO blocks (id, blocker_local_id, blocker_remote_id, blocked_local_id, blocked_remote_id)
            VALUES ($1, $2, $3, $4, $5)
            "#
        ).bind(block.id().as_ref())
            .bind(blocker_local_id)
            .bind(blocker_remote_id)
            .bind(blocked_local_id)
            .bind(blocked_remote_id)
            .execute(con)
            .await;
        match result {
            Ok(_) => Ok(true),
            Err(sqlx::Error::Database(db_err))
                if db_err.code().is_some_and(|code| code == "23505") =>
            {
                Ok(false)
            }
            Err(e) => Err(Report::from(e).change_context(KernelError::Internal)),
        }
    }

    async fn delete_if_exists(
        &self,
        executor: &mut Self::Connection,
        source: &BlockTargetId,
        destination: &BlockTargetId,
    ) -> error_stack::Result<bool, KernelError> {
        let con: &mut PgConnection = executor;
        let (blocker_local_id, blocker_remote_id) = split_block_target_id(source);
        let (blocked_local_id, blocked_remote_id) = split_block_target_id(destination);
        let result = sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM blocks
            WHERE blocker_local_id IS NOT DISTINCT FROM $1
              AND blocker_remote_id IS NOT DISTINCT FROM $2
              AND blocked_local_id IS NOT DISTINCT FROM $3
              AND blocked_remote_id IS NOT DISTINCT FROM $4
            "#,
        )
        .bind(blocker_local_id)
        .bind(blocker_remote_id)
        .bind(blocked_local_id)
        .bind(blocked_remote_id)
        .execute(con)
        .await
        .convert_error()?;
        Ok(result.rows_affected() > 0)
    }
}

impl DependOnBlockRepository for PostgresDatabase {
    type BlockRepository = PostgresBlockRepository;

    fn block_repository(&self) -> &Self::BlockRepository {
        &PostgresBlockRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
        use kernel::interfaces::repository::{BlockRepository, DependOnBlockRepository};
        use kernel::prelude::entity::{AccountId, BlockTargetId};
        use kernel::test_utils::{unique_account_name, AccountBuilder, BlockBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_blocks() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();
            let blocker_id = AccountId::default();
            let blocker_account = AccountBuilder::new()
                .id(blocker_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocker_account)
                .await
                .unwrap();
            let blocked_id = AccountId::default();
            let blocked_account = AccountBuilder::new()
                .id(blocked_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocked_account)
                .await
                .unwrap();
            let block = BlockBuilder::new()
                .source_local(blocker_id.clone())
                .destination_local(blocked_id.clone())
                .build();

            database
                .block_repository()
                .create(&mut conn, &block)
                .await
                .unwrap();

            let blocks = database
                .block_repository()
                .find_blocks(&mut conn, &BlockTargetId::from(blocker_id))
                .await
                .unwrap();
            assert_eq!(blocks[0].id(), block.id());

            let blocks = database
                .block_repository()
                .find_blocks(&mut conn, &BlockTargetId::from(blocked_id))
                .await
                .unwrap();
            assert!(blocks.is_empty());
            database
                .block_repository()
                .delete(&mut conn, block.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocker_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocked_account.id())
                .await
                .unwrap();
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
        use kernel::interfaces::repository::{BlockRepository, DependOnBlockRepository};
        use kernel::prelude::entity::{AccountId, BlockTargetId};
        use kernel::test_utils::{unique_account_name, AccountBuilder, BlockBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();
            let blocker_id = AccountId::default();
            let blocker_account = AccountBuilder::new()
                .id(blocker_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocker_account)
                .await
                .unwrap();
            let blocked_id = AccountId::default();
            let blocked_account = AccountBuilder::new()
                .id(blocked_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocked_account)
                .await
                .unwrap();
            let block = BlockBuilder::new()
                .source_local(blocker_id)
                .destination_local(blocked_id)
                .build();

            database
                .block_repository()
                .create(&mut conn, &block)
                .await
                .unwrap();
            database
                .block_repository()
                .delete(&mut conn, block.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocker_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocked_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();
            let blocker_id = AccountId::default();
            let blocker_account = AccountBuilder::new()
                .id(blocker_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocker_account)
                .await
                .unwrap();
            let blocked_id = AccountId::default();
            let blocked_account = AccountBuilder::new()
                .id(blocked_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocked_account)
                .await
                .unwrap();
            let block = BlockBuilder::new()
                .source_local(blocker_id.clone())
                .destination_local(blocked_id)
                .build();

            database
                .block_repository()
                .create(&mut conn, &block)
                .await
                .unwrap();

            database
                .block_repository()
                .delete(&mut conn, block.id())
                .await
                .unwrap();

            let blocks = database
                .block_repository()
                .find_blocks(&mut conn, &BlockTargetId::from(blocker_id))
                .await
                .unwrap();
            assert!(blocks.is_empty());
            database
                .account_read_model()
                .deactivate(&mut conn, blocker_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocked_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn insert_if_absent_returns_true_then_false_on_duplicate() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();
            let blocker_id = AccountId::default();
            let blocker_account = AccountBuilder::new()
                .id(blocker_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocker_account)
                .await
                .unwrap();
            let blocked_id = AccountId::default();
            let blocked_account = AccountBuilder::new()
                .id(blocked_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocked_account)
                .await
                .unwrap();
            let block = BlockBuilder::new()
                .source_local(blocker_id.clone())
                .destination_local(blocked_id.clone())
                .build();
            let duplicate = BlockBuilder::new()
                .source_local(blocker_id.clone())
                .destination_local(blocked_id.clone())
                .build();

            let inserted = database
                .block_repository()
                .insert_if_absent(&mut conn, &block)
                .await
                .unwrap();
            assert!(inserted);
            let inserted = database
                .block_repository()
                .insert_if_absent(&mut conn, &duplicate)
                .await
                .unwrap();
            assert!(!inserted);

            let blocks = database
                .block_repository()
                .find_blocks(&mut conn, &BlockTargetId::from(blocker_id.clone()))
                .await
                .unwrap();
            assert_eq!(blocks.len(), 1);
            database
                .block_repository()
                .delete(&mut conn, block.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocker_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocked_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete_if_exists_returns_true_when_present_false_when_absent() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();
            let blocker_id = AccountId::default();
            let blocker_account = AccountBuilder::new()
                .id(blocker_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocker_account)
                .await
                .unwrap();
            let blocked_id = AccountId::default();
            let blocked_account = AccountBuilder::new()
                .id(blocked_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &blocked_account)
                .await
                .unwrap();
            let other_blocked_id = AccountId::default();
            let other_blocked_account = AccountBuilder::new()
                .id(other_blocked_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut conn, &other_blocked_account)
                .await
                .unwrap();
            let block = BlockBuilder::new()
                .source_local(blocker_id.clone())
                .destination_local(blocked_id.clone())
                .build();
            let other_block = BlockBuilder::new()
                .source_local(blocker_id.clone())
                .destination_local(other_blocked_id.clone())
                .build();
            database
                .block_repository()
                .create(&mut conn, &block)
                .await
                .unwrap();
            database
                .block_repository()
                .create(&mut conn, &other_block)
                .await
                .unwrap();

            let deleted = database
                .block_repository()
                .delete_if_exists(
                    &mut conn,
                    &BlockTargetId::from(blocker_id.clone()),
                    &BlockTargetId::from(blocked_id.clone()),
                )
                .await
                .unwrap();
            assert!(deleted);
            let deleted = database
                .block_repository()
                .delete_if_exists(
                    &mut conn,
                    &BlockTargetId::from(blocker_id.clone()),
                    &BlockTargetId::from(blocked_id.clone()),
                )
                .await
                .unwrap();
            assert!(!deleted);

            let blocks = database
                .block_repository()
                .find_blocks(&mut conn, &BlockTargetId::from(blocker_id))
                .await
                .unwrap();
            assert_eq!(blocks.len(), 1);
            assert_eq!(blocks[0].id(), other_block.id());
            database
                .block_repository()
                .delete(&mut conn, other_block.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocker_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, blocked_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut conn, other_blocked_account.id())
                .await
                .unwrap();
        }
    }
}
