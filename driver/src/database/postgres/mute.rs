use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{DependOnMuteRepository, MuteRepository};
use kernel::prelude::entity::{AccountId, Mute, MuteId, MuteTargetId, RemoteAccountId};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct MuteRow {
    id: i64,
    muter_local_id: Option<i64>,
    muter_remote_id: Option<i64>,
    muted_local_id: Option<i64>,
    muted_remote_id: Option<i64>,
}

impl TryFrom<MuteRow> for Mute {
    type Error = Report<KernelError>;

    fn try_from(value: MuteRow) -> Result<Self, Self::Error> {
        let id = MuteId::new(value.id);
        let source = match (value.muter_local_id, value.muter_remote_id) {
            (Some(muter_local_id), None) => MuteTargetId::from(AccountId::new(muter_local_id)),
            (None, Some(muter_remote_id)) => {
                MuteTargetId::from(RemoteAccountId::new(muter_remote_id))
            }
            _ => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Invalid mute data. muter_local_id: {:?}, muter_remote_id: {:?}",
                    value.muter_local_id, value.muter_remote_id
                )))
            }
        };
        let destination = match (value.muted_local_id, value.muted_remote_id) {
            (Some(muted_local_id), None) => MuteTargetId::from(AccountId::new(muted_local_id)),
            (None, Some(muted_remote_id)) => {
                MuteTargetId::from(RemoteAccountId::new(muted_remote_id))
            }
            _ => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Invalid mute data. muted_local_id: {:?}, muted_remote_id: {:?}",
                    value.muted_local_id, value.muted_remote_id
                )))
            }
        };

        Mute::new(id, source, destination)
    }
}

pub struct PostgresMuteRepository;

fn split_mute_target_id(target_id: &MuteTargetId) -> (Option<&i64>, Option<&i64>) {
    match target_id {
        MuteTargetId::Local(account_id) => (Some(account_id.as_ref()), None),
        MuteTargetId::Remote(remote_account_id) => (None, Some(remote_account_id.as_ref())),
    }
}

impl MuteRepository for PostgresMuteRepository {
    type Executor = PostgresConnection;

    async fn find_mutes(
        &self,
        executor: &mut Self::Executor,
        source_id: &MuteTargetId,
    ) -> error_stack::Result<Vec<Mute>, KernelError> {
        let con: &mut PgConnection = executor;
        match source_id {
            MuteTargetId::Local(account_id) => {
                sqlx::query_as::<_, MuteRow>(
                    //language=postgresql
                    r#"
            SELECT id, muter_local_id, muter_remote_id, muted_local_id, muted_remote_id
            FROM mutes
            WHERE muter_local_id = $1
            "#,
                )
                .bind(account_id.as_ref())
            }
            MuteTargetId::Remote(remote_account_id) => {
                sqlx::query_as::<_, MuteRow>(
                    //language=postgresql
                    r#"
            SELECT id, muter_local_id, muter_remote_id, muted_local_id, muted_remote_id
            FROM mutes
            WHERE muter_remote_id = $1
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
                .map(Mute::try_from)
                .collect::<Result<_, _>>()
        })
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        mute: &Mute,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let (muter_local_id, muter_remote_id) = split_mute_target_id(mute.source());
        let (muted_local_id, muted_remote_id) = split_mute_target_id(mute.destination());
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO mutes (id, muter_local_id, muter_remote_id, muted_local_id, muted_remote_id)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(mute.id().as_ref())
        .bind(muter_local_id)
        .bind(muter_remote_id)
        .bind(muted_local_id)
        .bind(muted_remote_id)
        .execute(con)
        .await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_err))
                if db_err.code().is_some_and(|code| code == "23505") =>
            {
                Err(Report::new(KernelError::Rejected)
                    .attach_printable("Duplicate mute: this mute relationship already exists"))
            }
            Err(e) => Err(Report::from(e).change_context(KernelError::Internal)),
        }
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        mute_id: &MuteId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM mutes WHERE id = $1
            "#,
        )
        .bind(mute_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target mute not found for delete"));
        }
        Ok(())
    }
}

impl DependOnMuteRepository for PostgresDatabase {
    type MuteRepository = PostgresMuteRepository;

    fn mute_repository(&self) -> &Self::MuteRepository {
        &PostgresMuteRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
        use kernel::interfaces::repository::{DependOnMuteRepository, MuteRepository};
        use kernel::prelude::entity::{AccountId, MuteTargetId};
        use kernel::test_utils::{unique_account_name, AccountBuilder, MuteBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_mutes() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();
            let muter_id = AccountId::default();
            let muter_account = AccountBuilder::new()
                .id(muter_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &muter_account)
                .await
                .unwrap();
            let muted_id = AccountId::default();
            let muted_account = AccountBuilder::new()
                .id(muted_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &muted_account)
                .await
                .unwrap();
            let mute = MuteBuilder::new()
                .source_local(muter_id.clone())
                .destination_local(muted_id.clone())
                .build();

            database
                .mute_repository()
                .create(&mut transaction, &mute)
                .await
                .unwrap();

            let mutes = database
                .mute_repository()
                .find_mutes(&mut transaction, &MuteTargetId::from(muter_id))
                .await
                .unwrap();
            assert_eq!(mutes[0].id(), mute.id());

            let mutes = database
                .mute_repository()
                .find_mutes(&mut transaction, &MuteTargetId::from(muted_id))
                .await
                .unwrap();
            assert!(mutes.is_empty());
            database
                .mute_repository()
                .delete(&mut transaction, mute.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, muter_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, muted_account.id())
                .await
                .unwrap();
        }
    }

    mod modify {
        use crate::database::PostgresDatabase;
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
        use kernel::interfaces::repository::{DependOnMuteRepository, MuteRepository};
        use kernel::prelude::entity::{AccountId, MuteTargetId};
        use kernel::test_utils::{unique_account_name, AccountBuilder, MuteBuilder};

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();
            let muter_id = AccountId::default();
            let muter_account = AccountBuilder::new()
                .id(muter_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &muter_account)
                .await
                .unwrap();
            let muted_id = AccountId::default();
            let muted_account = AccountBuilder::new()
                .id(muted_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &muted_account)
                .await
                .unwrap();
            let mute = MuteBuilder::new()
                .source_local(muter_id)
                .destination_local(muted_id)
                .build();

            database
                .mute_repository()
                .create(&mut transaction, &mute)
                .await
                .unwrap();
            database
                .mute_repository()
                .delete(&mut transaction, mute.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, muter_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, muted_account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.get_executor().await.unwrap();
            let muter_id = AccountId::default();
            let muter_account = AccountBuilder::new()
                .id(muter_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &muter_account)
                .await
                .unwrap();
            let muted_id = AccountId::default();
            let muted_account = AccountBuilder::new()
                .id(muted_id.clone())
                .name(unique_account_name())
                .build();
            database
                .account_read_model()
                .create(&mut transaction, &muted_account)
                .await
                .unwrap();
            let mute = MuteBuilder::new()
                .source_local(muter_id.clone())
                .destination_local(muted_id)
                .build();

            database
                .mute_repository()
                .create(&mut transaction, &mute)
                .await
                .unwrap();

            database
                .mute_repository()
                .delete(&mut transaction, mute.id())
                .await
                .unwrap();

            let mutes = database
                .mute_repository()
                .find_mutes(&mut transaction, &MuteTargetId::from(muter_id))
                .await
                .unwrap();
            assert!(mutes.is_empty());
            database
                .account_read_model()
                .deactivate(&mut transaction, muter_account.id())
                .await
                .unwrap();
            database
                .account_read_model()
                .deactivate(&mut transaction, muted_account.id())
                .await
                .unwrap();
        }
    }
}
