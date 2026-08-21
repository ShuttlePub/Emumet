use crate::permission::{check_permission, instance_moderate};
use error_stack::Report;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnTransactionManager, TransactionManager,
};
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository};
use kernel::prelude::entity::{Account, AuthAccountId, ModerationReason, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait SuspendAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn suspend_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
        reason: String,
        expires_at: Option<time::OffsetDateTime>,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            ModerationReason::new(reason.as_str()).validate()?;
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query()
                .find_by_nanoid_unfiltered(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            if let Some(exp) = expires_at {
                if exp <= time::OffsetDateTime::now_utc() {
                    return Err(Report::new(KernelError::Rejected)
                        .attach_printable("expires_at must be in the future"));
                }
            }

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();

                        if !account.status().is_active() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is not active"));
                        }
                        if account.deleted_at().is_some() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is deactivated"));
                        }

                        deps.account_repository()
                            .save(
                                executor,
                                Account::suspend(account_id, reason, expires_at, current_version),
                            )
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            Ok(())
        }
    }
}

impl<T> SuspendAccountUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

pub trait UnsuspendAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn unsuspend_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query()
                .find_by_nanoid_unfiltered(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();

                        if !account.status().is_suspended() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is not suspended"));
                        }

                        deps.account_repository()
                            .save(executor, Account::unsuspend(account_id, current_version))
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            Ok(())
        }
    }
}

impl<T> UnsuspendAccountUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

pub trait BanAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn ban_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
        reason: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            ModerationReason::new(reason.as_str()).validate()?;
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query()
                .find_by_nanoid_unfiltered(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();

                        if account.status().is_banned() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is already banned"));
                        }
                        if account.deleted_at().is_some() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is deactivated"));
                        }

                        deps.account_repository()
                            .save(executor, Account::ban(account_id, reason, current_version))
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            Ok(())
        }
    }
}

impl<T> BanAccountUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

pub trait UnbanAccountUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnAccountRepository
    + DependOnTransactionManager
    + DependOnPermissionChecker
{
    fn unban_account<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send + 'a {
        async move {
            let mut conn = self.database_connection().connection().await?;

            let nanoid = Nanoid::<Account>::new(account_id);
            let projection = self
                .account_query()
                .find_by_nanoid_unfiltered(&mut conn, &nanoid)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "Account not found with nanoid: {}",
                        nanoid.as_ref()
                    ))
                })?;

            check_permission(self, auth_account_id, &instance_moderate()).await?;

            let account_id = projection.id().clone();
            let deps = self.clone();
            self.transaction_manager()
                .transaction(move |executor| {
                    Box::pin(async move {
                        let (account, current_version) = deps
                            .account_repository()
                            .load(executor, &account_id)
                            .await?
                            .into_parts();

                        if !account.status().is_banned() {
                            return Err(Report::new(KernelError::Rejected)
                                .attach_printable("Account is not banned"));
                        }

                        deps.account_repository()
                            .save(executor, Account::unban(account_id, current_version))
                            .await?;
                        Ok(())
                    })
                })
                .await?;

            Ok(())
        }
    }
}

impl<T> UnbanAccountUseCase for T where
    T: 'static
        + Clone
        + DependOnAccountQuery
        + DependOnAccountRepository
        + DependOnTransactionManager
        + DependOnPermissionChecker
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::database::{
        Connection, DependOnDatabaseConnection, DependOnTransactionManager,
    };
    use kernel::interfaces::permission::{InstanceRole, PermissionChecker, PermissionReq};
    use kernel::interfaces::repository::Rehydrated;
    use kernel::prelude::entity::{
        AccountEvent, AccountId, AccountName, AccountStatus, CommandEnvelope, DeletedAt,
        EventEnvelope, EventVersion,
    };
    use kernel::test_utils::AccountBuilder;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use time::OffsetDateTime;

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
            Box::pin(async move {
                let mut connection = self.connection().await?;
                operation(&mut connection).await
            })
        }
    }

    #[derive(Clone)]
    struct MockAccountQuery {
        account: Option<Account>,
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
                .filter(|account| account.id() == id)
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
                .filter(|account| account.name() == name)
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
                .filter(|account| account.nanoid() == nanoid)
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
                .filter(|account| nanoids.contains(account.nanoid()))
                .cloned()
                .into_iter()
                .collect())
        }

        async fn find_by_id_unfiltered(
            &self,
            executor: &mut Self::Connection,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_id(executor, id).await
        }

        async fn find_by_nanoid_unfiltered(
            &self,
            executor: &mut Self::Connection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_nanoid(executor, nanoid).await
        }

        async fn find_by_nanoids_unfiltered(
            &self,
            executor: &mut Self::Connection,
            nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            self.find_by_nanoids(executor, nanoids).await
        }

        async fn find_by_nanoid_including_deleted(
            &self,
            executor: &mut Self::Connection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_nanoid(executor, nanoid).await
        }

        async fn is_linked_including_deleted(
            &self,
            _executor: &mut Self::Connection,
            _auth_id: &AuthAccountId,
            _account_id: &AccountId,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(false)
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
                .as_ref()
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

    #[derive(Clone)]
    struct MockPermissionChecker {
        allowed: bool,
    }

    impl PermissionChecker for MockPermissionChecker {
        async fn check(
            &self,
            _subject: &AuthAccountId,
            _req: &PermissionReq,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(self.allowed)
        }

        async fn list_instance_roles(
            &self,
            _subject: &AuthAccountId,
        ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
            Ok(Vec::new())
        }
    }

    #[derive(Clone)]
    struct MockModule {
        database: MockDatabaseConnection,
        accounts: MockAccountQuery,
        account_repository: MockAccountRepository,
        permission_checker: MockPermissionChecker,
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

    impl DependOnAccountQuery for MockModule {
        type AccountQuery = MockAccountQuery;

        fn account_query(&self) -> &Self::AccountQuery {
            &self.accounts
        }
    }

    impl DependOnAccountRepository for MockModule {
        type AccountRepository = MockAccountRepository;

        fn account_repository(&self) -> &Self::AccountRepository {
            &self.account_repository
        }
    }

    impl DependOnPermissionChecker for MockModule {
        type PermissionChecker = MockPermissionChecker;

        fn permission_checker(&self) -> &Self::PermissionChecker {
            &self.permission_checker
        }
    }

    struct Fixture {
        module: MockModule,
        operator_id: AuthAccountId,
        nanoid: String,
    }

    fn fixture(account: Option<Account>, allowed: bool) -> Fixture {
        let saved_events = Arc::new(Mutex::new(Vec::new()));
        Fixture {
            module: MockModule {
                database: MockDatabaseConnection,
                accounts: MockAccountQuery {
                    account: account.clone(),
                },
                account_repository: MockAccountRepository {
                    account,
                    saved_events,
                },
                permission_checker: MockPermissionChecker { allowed },
            },
            operator_id: AuthAccountId::default(),
            nanoid: "target-account".to_string(),
        }
    }

    fn account(status: AccountStatus, deleted_at: Option<DeletedAt<Account>>) -> Account {
        kernel::ensure_generator_initialized();
        AccountBuilder::new()
            .nanoid(Nanoid::new("target-account".to_string()))
            .status(status)
            .deleted_at(deleted_at)
            .build()
    }

    fn saved_events(module: &MockModule) -> Vec<AccountEvent> {
        module
            .account_repository
            .saved_events
            .lock()
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn unban_saves_unbanned_event_for_banned_account() {
        let fixture = fixture(
            Some(account(
                AccountStatus::Banned {
                    reason: "x".into(),
                    banned_at: OffsetDateTime::now_utc(),
                },
                None,
            )),
            true,
        );

        fixture
            .module
            .unban_account(&fixture.operator_id, fixture.nanoid)
            .await
            .unwrap();

        assert_eq!(saved_events(&fixture.module), vec![AccountEvent::Unbanned]);
    }

    #[tokio::test]
    async fn unban_rejects_active_account_without_saving_event() {
        let fixture = fixture(Some(account(AccountStatus::Active, None)), true);

        let result = fixture
            .module
            .unban_account(&fixture.operator_id, fixture.nanoid)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Rejected
        );
        assert!(saved_events(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn unban_rejects_deactivated_account_without_saving_event() {
        let fixture = fixture(
            Some(account(
                AccountStatus::Active,
                Some(DeletedAt::<Account>::now()),
            )),
            true,
        );

        let result = fixture
            .module
            .unban_account(&fixture.operator_id, fixture.nanoid)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Rejected
        );
        assert!(saved_events(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn unban_returns_not_found_for_unknown_nanoid_without_saving_event() {
        let fixture = fixture(None, true);

        let result = fixture
            .module
            .unban_account(&fixture.operator_id, fixture.nanoid)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::NotFound
        );
        assert!(saved_events(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn unban_rejects_operator_without_permission_without_saving_event() {
        let fixture = fixture(
            Some(account(
                AccountStatus::Banned {
                    reason: "x".into(),
                    banned_at: OffsetDateTime::now_utc(),
                },
                None,
            )),
            false,
        );

        let result = fixture
            .module
            .unban_account(&fixture.operator_id, fixture.nanoid)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::PermissionDenied
        );
        assert!(saved_events(&fixture.module).is_empty());
    }
}
