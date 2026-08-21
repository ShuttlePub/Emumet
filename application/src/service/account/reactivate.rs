// allow: SIZE_OK — use case and its mandated in-file mock unit tests form one testable module.
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::permission::{
    AccountRelation, DependOnPermissionWriter, PermissionWriter, RelationTarget,
};
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository};
use kernel::prelude::entity::{Account, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait ReactivateAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionWriter
{
    fn reactivate_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query()
                .find_by_nanoid_including_deleted(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            if !self
                .account_query()
                .is_linked_including_deleted(&mut conn, auth_account_id, projection.id())
                .await?
            {
                return Err(Report::new(KernelError::PermissionDenied)
                    .attach_printable("Account is not linked to the authenticated principal"));
            }

            let account_id = projection.id().clone();
            let transaction_account_id = account_id.clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &transaction_account_id)
                            .await?
                            .into_parts();
                        if account.deleted_at().is_none() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is not deactivated"));
                        }
                        deps.account_repository()
                            .save(
                                executor,
                                Account::reactivate(transaction_account_id, current_version),
                            )
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            for relation in [
                AccountRelation::Owner,
                AccountRelation::Editor,
                AccountRelation::Signer,
            ] {
                self.permission_writer()
                    .create_relation(
                        &RelationTarget::Account {
                            account_id: account_id.clone(),
                            relation,
                        },
                        auth_account_id,
                    )
                    .await?;
            }

            Ok(())
        }
    }
}

impl<T> ReactivateAccountUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionWriter
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use error_stack::Report;
    use kernel::interfaces::database::{
        Connection, DatabaseConnection, DependOnDatabaseConnection, DependOnTransactionManager,
        TransactionManager,
    };
    use kernel::interfaces::permission::{
        AccountRelation, DependOnPermissionWriter, PermissionWriter, RelationTarget,
    };
    use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
    use kernel::interfaces::repository::{
        AggregateRepository, DependOnAccountRepository, Rehydrated,
    };
    use kernel::prelude::entity::{
        Account, AccountEvent, AccountId, AccountName, AuthAccountId, CommandEnvelope, DeletedAt,
        EventEnvelope, EventVersion, Nanoid,
    };
    use kernel::test_utils::AccountBuilder;
    use kernel::KernelError;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct MockConnection;

    impl Connection for MockConnection {}

    #[derive(Clone)]
    struct MockDatabase;

    impl DatabaseConnection for MockDatabase {
        type Connection = MockConnection;

        async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
            Ok(MockConnection)
        }
    }

    impl TransactionManager for MockDatabase {
        fn transaction<'a, F, T>(
            &'a self,
            operation: F,
        ) -> Pin<Box<dyn Future<Output = error_stack::Result<T, KernelError>> + Send + 'a>>
        where
            F: for<'connection> FnOnce(
                    &'connection mut Self::Connection,
                ) -> Pin<
                    Box<
                        dyn Future<Output = error_stack::Result<T, KernelError>>
                            + Send
                            + 'connection,
                    >,
                > + Send
                + 'a,
            T: Send + 'a,
        {
            Box::pin(async move { operation(&mut MockConnection).await })
        }
    }

    #[derive(Clone)]
    struct MockAccountQuery {
        account: Option<Account>,
        linked: bool,
    }

    impl AccountQuery for MockAccountQuery {
        type Connection = MockConnection;

        async fn find_by_id(
            &self,
            _executor: &mut Self::Connection,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| account.id() == id && account.deleted_at().is_none())
                .cloned())
        }

        async fn find_by_auth_id(
            &self,
            _executor: &mut Self::Connection,
            _auth_id: &AuthAccountId,
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(Vec::new())
        }

        async fn find_auth_account_id_by_account_id(
            &self,
            _executor: &mut Self::Connection,
            _account_id: &AccountId,
        ) -> error_stack::Result<Option<AuthAccountId>, KernelError> {
            Ok(None)
        }

        async fn find_by_name(
            &self,
            _executor: &mut Self::Connection,
            name: &AccountName,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| account.name() == name && account.deleted_at().is_none())
                .cloned())
        }

        async fn find_by_nanoid(
            &self,
            _executor: &mut Self::Connection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| account.nanoid() == nanoid && account.deleted_at().is_none())
                .cloned())
        }

        async fn find_by_nanoids(
            &self,
            _executor: &mut Self::Connection,
            nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| {
                    nanoids.contains(account.nanoid()) && account.deleted_at().is_none()
                })
                .cloned()
                .into_iter()
                .collect())
        }

        async fn find_by_id_unfiltered(
            &self,
            _executor: &mut Self::Connection,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| account.id() == id)
                .cloned())
        }

        async fn find_by_nanoid_unfiltered(
            &self,
            _executor: &mut Self::Connection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| account.nanoid() == nanoid)
                .cloned())
        }

        async fn find_by_nanoids_unfiltered(
            &self,
            _executor: &mut Self::Connection,
            nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| nanoids.contains(account.nanoid()))
                .cloned()
                .into_iter()
                .collect())
        }

        async fn find_by_nanoid_including_deleted(
            &self,
            executor: &mut Self::Connection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_nanoid_unfiltered(executor, nanoid).await
        }

        async fn is_linked_including_deleted(
            &self,
            _executor: &mut Self::Connection,
            _auth_id: &AuthAccountId,
            account_id: &AccountId,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(self.linked
                && self
                    .account
                    .as_ref()
                    .is_some_and(|account| account.id() == account_id))
        }
    }

    #[derive(Clone)]
    struct MockAccountRepository {
        account: Option<Account>,
        saved_events: Arc<Mutex<Vec<AccountEvent>>>,
    }

    impl AggregateRepository<Account> for MockAccountRepository {
        type Connection = MockConnection;
        type Id = AccountId;

        async fn load(
            &self,
            _executor: &mut Self::Connection,
            _id: &Self::Id,
        ) -> error_stack::Result<Rehydrated<Account>, KernelError> {
            self.account
                .clone()
                .map(|account| Rehydrated::new(account.clone(), account.version().clone()))
                .ok_or_else(|| Report::new(KernelError::NotFound))
        }

        async fn save(
            &self,
            _executor: &mut Self::Connection,
            command: CommandEnvelope<AccountEvent, Account>,
        ) -> error_stack::Result<EventEnvelope<AccountEvent, Account>, KernelError> {
            self.saved_events
                .lock()
                .unwrap()
                .push(command.event().clone());
            Ok(EventEnvelope::new(
                command.id().clone(),
                command.event().clone(),
                EventVersion::default(),
            ))
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum WriterCall {
        Create {
            account_id: AccountId,
            relation: AccountRelation,
            subject: AuthAccountId,
        },
        Delete,
    }

    #[derive(Clone, Default)]
    struct MockPermissionWriter {
        calls: Arc<Mutex<Vec<WriterCall>>>,
    }

    impl PermissionWriter for MockPermissionWriter {
        async fn create_relation(
            &self,
            target: &RelationTarget,
            subject: &AuthAccountId,
        ) -> error_stack::Result<(), KernelError> {
            let RelationTarget::Account {
                account_id,
                relation,
            } = target
            else {
                panic!("expected account relation target");
            };
            self.calls.lock().unwrap().push(WriterCall::Create {
                account_id: account_id.clone(),
                relation: *relation,
                subject: subject.clone(),
            });
            Ok(())
        }

        async fn delete_relation(
            &self,
            _target: &RelationTarget,
            _subject: &AuthAccountId,
        ) -> error_stack::Result<(), KernelError> {
            self.calls.lock().unwrap().push(WriterCall::Delete);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct MockModule {
        database: MockDatabase,
        query: MockAccountQuery,
        repository: MockAccountRepository,
        permission_writer: MockPermissionWriter,
    }

    impl DependOnDatabaseConnection for MockModule {
        type DatabaseConnection = MockDatabase;

        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.database
        }
    }

    impl DependOnTransactionManager for MockModule {
        type TransactionManager = MockDatabase;

        fn transaction_manager(&self) -> &Self::TransactionManager {
            &self.database
        }
    }

    impl DependOnAccountQuery for MockModule {
        type AccountQuery = MockAccountQuery;

        fn account_query(&self) -> &Self::AccountQuery {
            &self.query
        }
    }

    impl DependOnAccountRepository for MockModule {
        type AccountRepository = MockAccountRepository;

        fn account_repository(&self) -> &Self::AccountRepository {
            &self.repository
        }
    }

    impl DependOnPermissionWriter for MockModule {
        type PermissionWriter = MockPermissionWriter;

        fn permission_writer(&self) -> &Self::PermissionWriter {
            &self.permission_writer
        }
    }

    struct Fixture {
        module: MockModule,
        auth_account_id: AuthAccountId,
        account_id: AccountId,
        nanoid: String,
    }

    fn fixture(account: Option<Account>, linked: bool) -> Fixture {
        kernel::ensure_generator_initialized();
        let account_id = account
            .as_ref()
            .map_or_else(AccountId::default, |account| account.id().clone());
        let nanoid = account.as_ref().map_or_else(
            || "unknown-account".to_string(),
            |account| account.nanoid().as_ref().clone(),
        );
        let saved_events = Arc::new(Mutex::new(Vec::new()));
        Fixture {
            module: MockModule {
                database: MockDatabase,
                query: MockAccountQuery {
                    account: account.clone(),
                    linked,
                },
                repository: MockAccountRepository {
                    account,
                    saved_events,
                },
                permission_writer: MockPermissionWriter::default(),
            },
            auth_account_id: AuthAccountId::default(),
            account_id,
            nanoid,
        }
    }

    fn account(deactivated: bool) -> Account {
        let builder = AccountBuilder::new().nanoid(Nanoid::new("target-account".to_string()));
        if deactivated {
            builder.deleted_at(Some(DeletedAt::now())).build()
        } else {
            builder.build()
        }
    }

    fn saved_events(module: &MockModule) -> Vec<AccountEvent> {
        module.repository.saved_events.lock().unwrap().clone()
    }

    fn writer_calls(module: &MockModule) -> Vec<WriterCall> {
        module.permission_writer.calls.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn reactivate_restores_deactivated_linked_account() {
        // Given
        let fixture = fixture(Some(account(true)), true);

        // When
        let result = fixture
            .module
            .reactivate_account(&fixture.auth_account_id, fixture.nanoid)
            .await;

        // Then
        assert!(result.is_ok());
        assert_eq!(
            saved_events(&fixture.module),
            vec![AccountEvent::Reactivated]
        );
        assert_eq!(
            writer_calls(&fixture.module),
            [
                AccountRelation::Owner,
                AccountRelation::Editor,
                AccountRelation::Signer,
            ]
            .into_iter()
            .map(|relation| WriterCall::Create {
                account_id: fixture.account_id.clone(),
                relation,
                subject: fixture.auth_account_id.clone(),
            })
            .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn reactivate_rejects_account_that_is_not_deactivated() {
        // Given
        let fixture = fixture(Some(account(false)), true);

        // When
        let result = fixture
            .module
            .reactivate_account(&fixture.auth_account_id, fixture.nanoid)
            .await;

        // Then
        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Rejected
        );
        assert!(saved_events(&fixture.module).is_empty());
        assert!(writer_calls(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn reactivate_denies_account_not_linked_to_principal() {
        // Given
        let fixture = fixture(Some(account(true)), false);

        // When
        let result = fixture
            .module
            .reactivate_account(&fixture.auth_account_id, fixture.nanoid)
            .await;

        // Then
        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::PermissionDenied
        );
        assert!(saved_events(&fixture.module).is_empty());
        assert!(writer_calls(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn reactivate_returns_not_found_for_unknown_nanoid() {
        // Given
        let fixture = fixture(None, true);

        // When
        let result = fixture
            .module
            .reactivate_account(&fixture.auth_account_id, fixture.nanoid)
            .await;

        // Then
        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::NotFound
        );
        assert!(writer_calls(&fixture.module).is_empty());
    }
}
