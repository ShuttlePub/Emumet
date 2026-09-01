use crate::dto::account::AccountDto;
use crate::dto::pagination::{apply_pagination, Pagination};
use crate::permission::{account_view, check_permission};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::DependOnPermissionChecker;
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::prelude::entity::{Account, AccountKind, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait GetAccountUseCase:
    'static + Sync + Send + DependOnAccountQuery + DependOnPermissionChecker
{
    // find_by_auth_id returns only accounts owned by the authenticated user,
    // so no additional permission check is needed.
    fn get_all_accounts(
        &self,
        auth_account_id: &AuthAccountId,
        Pagination {
            direction,
            cursor,
            limit,
        }: Pagination<String>,
    ) -> impl Future<Output = error_stack::Result<Option<Vec<AccountDto>>, KernelError>> + Send
    {
        async move {
            let mut conn = self.database_connection().connection().await?;
            let accounts = self
                .account_query()
                .find_by_auth_id(&mut conn, auth_account_id)
                .await?
                .into_iter()
                .filter(|account| account.kind() == &AccountKind::Personal)
                .collect();
            let cursor = if let Some(cursor) = cursor {
                let id: Nanoid<Account> = Nanoid::new(cursor);
                self.account_query().find_by_nanoid(&mut conn, &id).await?
            } else {
                None
            };
            let accounts = apply_pagination(accounts, limit, cursor, direction);
            Ok(Some(accounts.into_iter().map(AccountDto::from).collect()))
        }
    }

    fn get_accounts_by_ids(
        &self,
        auth_account_id: &AuthAccountId,
        ids: Vec<String>,
    ) -> impl Future<Output = error_stack::Result<Vec<AccountDto>, KernelError>> + Send {
        async move {
            let mut conn = self.database_connection().connection().await?;

            let nanoids: Vec<Nanoid<Account>> =
                ids.into_iter().map(Nanoid::<Account>::new).collect();
            let accounts = self
                .account_query()
                .find_by_nanoids(&mut conn, &nanoids)
                .await?;

            let mut result = Vec::new();
            for account in accounts {
                if check_permission(self, auth_account_id, &account_view(account.id()))
                    .await
                    .is_ok()
                {
                    result.push(AccountDto::from(account));
                }
            }

            Ok(result)
        }
    }
}

impl<T> GetAccountUseCase for T where T: 'static + DependOnAccountQuery + DependOnPermissionChecker {}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::database::{Connection, DependOnDatabaseConnection};
    use kernel::interfaces::permission::{InstanceRole, PermissionChecker, PermissionReq};
    use kernel::prelude::entity::{AccountId, AccountName};
    use kernel::test_utils::AccountBuilder;

    struct MockConnection;
    impl Connection for MockConnection {}

    struct MockDatabase;
    impl kernel::interfaces::database::DatabaseConnection for MockDatabase {
        type Connection = MockConnection;
        async fn connection(&self) -> error_stack::Result<MockConnection, KernelError> {
            Ok(MockConnection)
        }
    }

    struct MockAccounts(Vec<Account>);
    impl AccountQuery for MockAccounts {
        type Connection = MockConnection;
        async fn find_by_id(
            &self,
            _: &mut MockConnection,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self.0.iter().find(|account| account.id() == id).cloned())
        }
        async fn find_by_auth_id(
            &self,
            _: &mut MockConnection,
            _: &AuthAccountId,
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(self.0.clone())
        }
        async fn find_auth_account_id_by_account_id(
            &self,
            _: &mut MockConnection,
            _: &AccountId,
        ) -> error_stack::Result<Option<AuthAccountId>, KernelError> {
            Ok(None)
        }
        async fn find_by_name(
            &self,
            _: &mut MockConnection,
            name: &AccountName,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self
                .0
                .iter()
                .find(|account| account.name() == name)
                .cloned())
        }
        async fn find_by_nanoid(
            &self,
            _: &mut MockConnection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok(self
                .0
                .iter()
                .find(|account| account.nanoid() == nanoid)
                .cloned())
        }
        async fn find_by_nanoids(
            &self,
            _: &mut MockConnection,
            nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(self
                .0
                .iter()
                .filter(|account| nanoids.contains(account.nanoid()))
                .cloned()
                .collect())
        }
        async fn find_by_id_unfiltered(
            &self,
            executor: &mut MockConnection,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_id(executor, id).await
        }
        async fn find_by_nanoid_unfiltered(
            &self,
            executor: &mut MockConnection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_nanoid(executor, nanoid).await
        }
        async fn find_by_nanoids_unfiltered(
            &self,
            executor: &mut MockConnection,
            nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            self.find_by_nanoids(executor, nanoids).await
        }
        async fn find_by_nanoid_including_deleted(
            &self,
            executor: &mut MockConnection,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_nanoid(executor, nanoid).await
        }
        async fn is_linked_including_deleted(
            &self,
            _: &mut MockConnection,
            _: &AuthAccountId,
            _: &AccountId,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(false)
        }
    }

    struct AllowPermissions;
    impl PermissionChecker for AllowPermissions {
        async fn check(
            &self,
            _: &AuthAccountId,
            _: &PermissionReq,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(true)
        }
        async fn list_instance_roles(
            &self,
            _: &AuthAccountId,
        ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
            Ok(Vec::new())
        }
    }

    struct Module {
        database: MockDatabase,
        accounts: MockAccounts,
        permissions: AllowPermissions,
    }
    impl DependOnDatabaseConnection for Module {
        type DatabaseConnection = MockDatabase;
        fn database_connection(&self) -> &MockDatabase {
            &self.database
        }
    }
    impl DependOnAccountQuery for Module {
        type AccountQuery = MockAccounts;
        fn account_query(&self) -> &MockAccounts {
            &self.accounts
        }
    }
    impl DependOnPermissionChecker for Module {
        type PermissionChecker = AllowPermissions;
        fn permission_checker(&self) -> &AllowPermissions {
            &self.permissions
        }
    }

    #[tokio::test]
    async fn get_all_accounts_excludes_organization_accounts() {
        kernel::ensure_generator_initialized();
        let personal = AccountBuilder::new()
            .nanoid(Nanoid::new("personal"))
            .build();
        let organization = AccountBuilder::new()
            .kind(AccountKind::Organization)
            .nanoid(Nanoid::new("organization"))
            .build();
        let module = Module {
            database: MockDatabase,
            accounts: MockAccounts(vec![personal, organization]),
            permissions: AllowPermissions,
        };

        let result = module
            .get_all_accounts(
                &AuthAccountId::default(),
                Pagination::new(None, None, Default::default()),
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].nanoid, "personal");
    }
}
