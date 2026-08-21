use crate::permission::{check_permission, instance_administrate};
use error_stack::Report;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::{
    DependOnPermissionChecker, DependOnPermissionWriter, InstanceRole, PermissionWriter,
    RelationTarget,
};
use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
use kernel::prelude::entity::{Account, AuthAccountId, Nanoid};
use kernel::KernelError;
use std::future::Future;

pub trait AssignInstanceRoleUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnPermissionChecker
    + DependOnPermissionWriter
{
    fn assign_instance_role<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
        role: InstanceRole,
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

            check_permission(self, auth_account_id, &instance_administrate()).await?;

            let target_auth_id = self
                .account_query()
                .find_auth_account_id_by_account_id(&mut conn, projection.id())
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "No auth account linked to account: {:?}",
                        projection.id()
                    ))
                })?;

            self.permission_writer()
                .create_relation(&RelationTarget::Instance { role }, &target_auth_id)
                .await
        }
    }
}

impl<T> AssignInstanceRoleUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnPermissionChecker
        + DependOnPermissionWriter
{
}

pub trait RevokeInstanceRoleUseCase:
    'static
    + Sync
    + Send
    + Clone
    + DependOnAccountQuery
    + DependOnPermissionChecker
    + DependOnPermissionWriter
{
    fn revoke_instance_role<'a>(
        &'a self,
        auth_account_id: &'a AuthAccountId,
        account_id: String,
        role: InstanceRole,
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

            check_permission(self, auth_account_id, &instance_administrate()).await?;

            let target_auth_id = self
                .account_query()
                .find_auth_account_id_by_account_id(&mut conn, projection.id())
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound).attach_printable(format!(
                        "No auth account linked to account: {:?}",
                        projection.id()
                    ))
                })?;

            if role == InstanceRole::Admin && &target_auth_id == auth_account_id {
                return Err(Report::new(KernelError::Rejected)
                    .attach_printable("Cannot revoke your own admin role"));
            }

            self.permission_writer()
                .delete_relation(&RelationTarget::Instance { role }, &target_auth_id)
                .await
        }
    }
}

impl<T> RevokeInstanceRoleUseCase for T where
    T: 'static
        + Sync
        + Send
        + Clone
        + DependOnAccountQuery
        + DependOnPermissionChecker
        + DependOnPermissionWriter
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::database::{
        Connection, DatabaseConnection, DependOnDatabaseConnection,
    };
    use kernel::interfaces::permission::{
        PermissionChecker, PermissionReq, PermissionWriter, RelationTarget,
    };
    use kernel::interfaces::read_model::{AccountQuery, DependOnAccountQuery};
    use kernel::prelude::entity::{Account, AccountId, AccountName, Nanoid};
    use kernel::test_utils::AccountBuilder;
    use std::sync::{Arc, Mutex};

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

    #[derive(Clone)]
    struct MockAccountQuery {
        account: Option<Account>,
        linked_auth_account_id: Option<AuthAccountId>,
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
            account_id: &AccountId,
        ) -> error_stack::Result<Option<AuthAccountId>, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| account.id() == account_id)
                .and(self.linked_auth_account_id.clone()))
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
            auth_id: &AuthAccountId,
            account_id: &AccountId,
        ) -> error_stack::Result<bool, KernelError> {
            Ok(self
                .account
                .as_ref()
                .filter(|account| account.id() == account_id)
                .is_some()
                && self.linked_auth_account_id.as_ref() == Some(auth_id))
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

    #[derive(Clone, Debug, PartialEq)]
    enum WriterCall {
        Create {
            target_role: InstanceRole,
            subject: AuthAccountId,
        },
        Delete {
            target_role: InstanceRole,
            subject: AuthAccountId,
        },
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
            let RelationTarget::Instance { role } = target else {
                panic!("expected instance relation target");
            };
            self.calls.lock().unwrap().push(WriterCall::Create {
                target_role: *role,
                subject: subject.clone(),
            });
            Ok(())
        }

        async fn delete_relation(
            &self,
            target: &RelationTarget,
            subject: &AuthAccountId,
        ) -> error_stack::Result<(), KernelError> {
            let RelationTarget::Instance { role } = target else {
                panic!("expected instance relation target");
            };
            self.calls.lock().unwrap().push(WriterCall::Delete {
                target_role: *role,
                subject: subject.clone(),
            });
            Ok(())
        }
    }

    #[derive(Clone)]
    struct MockModule {
        database: MockDatabaseConnection,
        accounts: MockAccountQuery,
        permission_checker: MockPermissionChecker,
        permission_writer: MockPermissionWriter,
    }

    impl DependOnDatabaseConnection for MockModule {
        type DatabaseConnection = MockDatabaseConnection;

        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.database
        }
    }

    impl DependOnAccountQuery for MockModule {
        type AccountQuery = MockAccountQuery;

        fn account_query(&self) -> &Self::AccountQuery {
            &self.accounts
        }
    }

    impl DependOnPermissionChecker for MockModule {
        type PermissionChecker = MockPermissionChecker;

        fn permission_checker(&self) -> &Self::PermissionChecker {
            &self.permission_checker
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
        operator_id: AuthAccountId,
        target_auth_id: AuthAccountId,
        nanoid: String,
    }

    fn fixture(account_exists: bool, linked_auth_exists: bool, allowed: bool) -> Fixture {
        kernel::ensure_generator_initialized();
        let nanoid = "target-account".to_string();
        let account = AccountBuilder::new()
            .nanoid(Nanoid::new(nanoid.clone()))
            .build();
        let operator_id = AuthAccountId::default();
        let target_auth_id = AuthAccountId::default();
        Fixture {
            module: MockModule {
                database: MockDatabaseConnection,
                accounts: MockAccountQuery {
                    account: account_exists.then_some(account),
                    linked_auth_account_id: linked_auth_exists.then_some(target_auth_id.clone()),
                },
                permission_checker: MockPermissionChecker { allowed },
                permission_writer: MockPermissionWriter::default(),
            },
            operator_id,
            target_auth_id,
            nanoid,
        }
    }

    fn calls(module: &MockModule) -> Vec<WriterCall> {
        module.permission_writer.calls.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn assign_creates_relation_for_target_auth_account() {
        let fixture = fixture(true, true, true);

        fixture
            .module
            .assign_instance_role(
                &fixture.operator_id,
                fixture.nanoid,
                InstanceRole::Moderator,
            )
            .await
            .unwrap();

        assert_eq!(
            calls(&fixture.module),
            vec![WriterCall::Create {
                target_role: InstanceRole::Moderator,
                subject: fixture.target_auth_id,
            }]
        );
    }

    #[tokio::test]
    async fn revoke_deletes_relation_for_target_auth_account() {
        let fixture = fixture(true, true, true);

        fixture
            .module
            .revoke_instance_role(&fixture.operator_id, fixture.nanoid, InstanceRole::Admin)
            .await
            .unwrap();

        assert_eq!(
            calls(&fixture.module),
            vec![WriterCall::Delete {
                target_role: InstanceRole::Admin,
                subject: fixture.target_auth_id,
            }]
        );
    }

    #[tokio::test]
    async fn assign_rejects_operator_without_administrate_permission() {
        let fixture = fixture(true, true, false);

        let result = fixture
            .module
            .assign_instance_role(&fixture.operator_id, fixture.nanoid, InstanceRole::Admin)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::PermissionDenied
        );
        assert!(calls(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn assign_returns_not_found_for_unknown_nanoid() {
        let fixture = fixture(false, true, true);

        let result = fixture
            .module
            .assign_instance_role(&fixture.operator_id, fixture.nanoid, InstanceRole::Admin)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::NotFound
        );
        assert!(calls(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn assign_returns_not_found_when_account_has_no_linked_auth_account() {
        let fixture = fixture(true, false, true);

        let result = fixture
            .module
            .assign_instance_role(&fixture.operator_id, fixture.nanoid, InstanceRole::Admin)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::NotFound
        );
        assert!(calls(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn revoke_rejects_own_admin_role() {
        let mut fixture = fixture(true, true, true);
        fixture.operator_id = fixture.target_auth_id.clone();

        let result = fixture
            .module
            .revoke_instance_role(&fixture.operator_id, fixture.nanoid, InstanceRole::Admin)
            .await;

        assert_eq!(
            result.unwrap_err().current_context(),
            &KernelError::Rejected
        );
        assert!(calls(&fixture.module).is_empty());
    }

    #[tokio::test]
    async fn revoke_allows_own_moderator_role() {
        let mut fixture = fixture(true, true, true);
        fixture.operator_id = fixture.target_auth_id.clone();

        fixture
            .module
            .revoke_instance_role(
                &fixture.operator_id,
                fixture.nanoid,
                InstanceRole::Moderator,
            )
            .await
            .unwrap();

        assert_eq!(
            calls(&fixture.module),
            vec![WriterCall::Delete {
                target_role: InstanceRole::Moderator,
                subject: fixture.target_auth_id,
            }]
        );
    }

    #[tokio::test]
    async fn assign_twice_creates_relation_twice() {
        let fixture = fixture(true, true, true);

        fixture
            .module
            .assign_instance_role(
                &fixture.operator_id,
                fixture.nanoid.clone(),
                InstanceRole::Admin,
            )
            .await
            .unwrap();
        fixture
            .module
            .assign_instance_role(&fixture.operator_id, fixture.nanoid, InstanceRole::Admin)
            .await
            .unwrap();

        assert_eq!(
            calls(&fixture.module),
            vec![
                WriterCall::Create {
                    target_role: InstanceRole::Admin,
                    subject: fixture.target_auth_id.clone(),
                },
                WriterCall::Create {
                    target_role: InstanceRole::Admin,
                    subject: fixture.target_auth_id,
                },
            ]
        );
    }
}
