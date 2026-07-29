use kernel::interfaces::permission::{DependOnPermissionChecker, InstanceRole, PermissionChecker};
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use std::future::Future;

#[derive(Debug, Clone, PartialEq)]
pub struct SessionContext {
    pub auth_account_id: AuthAccountId,
    pub instance_roles: Vec<InstanceRole>,
}

pub trait GetSessionContextUseCase: 'static + DependOnPermissionChecker {
    fn get_session_context(
        &self,
        auth_account_id: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<SessionContext, KernelError>> {
        async move {
            let roles = self
                .permission_checker()
                .list_instance_roles(auth_account_id)
                .await?;
            Ok(SessionContext {
                auth_account_id: auth_account_id.clone(),
                instance_roles: roles,
            })
        }
    }
}

impl<T: 'static + DependOnPermissionChecker> GetSessionContextUseCase for T {}

#[cfg(test)]
mod tests {
    use kernel::interfaces::permission::{
        DependOnPermissionChecker, InstanceRole, PermissionChecker,
    };
    use kernel::prelude::entity::AuthAccountId;
    use kernel::KernelError;
    use std::future::Future;

    #[derive(Debug, Clone)]
    enum MockBehavior {
        Roles(Vec<InstanceRole>),
        Error,
    }

    #[derive(Debug, Clone)]
    struct MockPermissionChecker {
        behavior: MockBehavior,
    }

    impl PermissionChecker for MockPermissionChecker {
        fn check(
            &self,
            _subject: &AuthAccountId,
            _req: &kernel::interfaces::permission::PermissionReq,
        ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send {
            async { Ok(true) }
        }

        fn list_instance_roles(
            &self,
            _subject: &AuthAccountId,
        ) -> impl Future<Output = error_stack::Result<Vec<InstanceRole>, KernelError>> + Send
        {
            let behavior = self.behavior.clone();
            async move {
                match behavior {
                    MockBehavior::Roles(roles) => Ok(roles),
                    MockBehavior::Error => Err(error_stack::Report::new(KernelError::Internal)),
                }
            }
        }
    }

    struct TestDeps {
        permission_checker: MockPermissionChecker,
    }

    impl DependOnPermissionChecker for TestDeps {
        type PermissionChecker = MockPermissionChecker;

        fn permission_checker(&self) -> &Self::PermissionChecker {
            &self.permission_checker
        }
    }

    /// 1. get_session_context が AuthAccountId と roles を含む SessionContext を返す
    #[tokio::test]
    async fn returns_session_context_with_roles() {
        kernel::ensure_generator_initialized();
        let deps = TestDeps {
            permission_checker: MockPermissionChecker {
                behavior: MockBehavior::Roles(vec![InstanceRole::Admin, InstanceRole::Moderator]),
            },
        };
        let auth_account_id = AuthAccountId::default();
        let result =
            super::GetSessionContextUseCase::get_session_context(&deps, &auth_account_id).await;
        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert_eq!(ctx.auth_account_id, auth_account_id);
        assert_eq!(
            ctx.instance_roles,
            vec![InstanceRole::Admin, InstanceRole::Moderator]
        );
    }

    /// 2. double が [Admin] のみ返す場合、結果も [Admin] のみ (暗黙包含なし)
    #[tokio::test]
    async fn returns_exact_roles_no_implicit_inclusion() {
        kernel::ensure_generator_initialized();
        let deps = TestDeps {
            permission_checker: MockPermissionChecker {
                behavior: MockBehavior::Roles(vec![InstanceRole::Admin]),
            },
        };
        let auth_account_id = AuthAccountId::default();
        let result =
            super::GetSessionContextUseCase::get_session_context(&deps, &auth_account_id).await;
        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert_eq!(ctx.instance_roles, vec![InstanceRole::Admin]);
        assert!(!ctx.instance_roles.contains(&InstanceRole::Moderator));
    }

    /// 3. double が Err を返す場合、use case も Err を返す (空 vec へのフォールバックなし)
    #[tokio::test]
    async fn propagates_error() {
        kernel::ensure_generator_initialized();
        let deps = TestDeps {
            permission_checker: MockPermissionChecker {
                behavior: MockBehavior::Error,
            },
        };
        let auth_account_id = AuthAccountId::default();
        let result =
            super::GetSessionContextUseCase::get_session_context(&deps, &auth_account_id).await;
        assert!(result.is_err());
    }

    /// 4. DB/read-model への依存がないことの証明:
    ///    TestDeps は DependOnPermissionChecker を impl するが DB 系 trait は impl しない。
    ///    → GetSessionContextUseCase が DependOnPermissionChecker のみ要求する設計であることをテストで表現。
    #[tokio::test]
    async fn only_depends_on_permission_checker() {
        kernel::ensure_generator_initialized();
        let deps = TestDeps {
            permission_checker: MockPermissionChecker {
                behavior: MockBehavior::Roles(vec![]),
            },
        };
        let auth_account_id = AuthAccountId::default();
        let result =
            super::GetSessionContextUseCase::get_session_context(&deps, &auth_account_id).await;
        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert!(ctx.instance_roles.is_empty());
    }
}
