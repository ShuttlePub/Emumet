use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::activitypub::{ActorUrlBuilder, OrderedCollection};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{DependOnFollowRepository, FollowRepository};
use kernel::prelude::entity::{AccountId, FollowTargetId};
use kernel::KernelError;
use std::future::Future;

pub trait GetFollowersCollectionUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnFollowRepository
    + DependOnPublicBaseUrl
{
    fn get_followers_collection(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<OrderedCollection, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let account = self
                .account_query_processor()
                .find_by_id(&mut executor, account_id)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Account not found: {account_id:?}"))
                })?;
            let follows = self
                .follow_repository()
                .find_followers(&mut executor, &FollowTargetId::from(account_id.clone()))
                .await?;
            let total_items = follows
                .iter()
                .filter(|follow| follow.approved_at().is_some())
                .count() as u64;

            Ok(OrderedCollection::new(
                ActorUrlBuilder::new(self.public_base_url().as_str(), account.nanoid().as_ref())
                    .followers(),
                total_items,
                None,
                None,
            ))
        }
    }

    fn get_following_collection(
        &self,
        account_id: &AccountId,
    ) -> impl Future<Output = error_stack::Result<OrderedCollection, KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            let account = self
                .account_query_processor()
                .find_by_id(&mut executor, account_id)
                .await?
                .ok_or_else(|| {
                    Report::new(KernelError::NotFound)
                        .attach_printable(format!("Account not found: {account_id:?}"))
                })?;
            let follows = self
                .follow_repository()
                .find_followings(&mut executor, &FollowTargetId::from(account_id.clone()))
                .await?;
            let total_items = follows
                .iter()
                .filter(|follow| follow.approved_at().is_some())
                .count() as u64;

            Ok(OrderedCollection::new(
                ActorUrlBuilder::new(self.public_base_url().as_str(), account.nanoid().as_ref())
                    .following(),
                total_items,
                None,
                None,
            ))
        }
    }
}

impl<T> GetFollowersCollectionUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnFollowRepository
        + DependOnPublicBaseUrl
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::config::PublicBaseUrl;
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
    use kernel::prelude::entity::{
        Account, AccountName, AuthAccountId, Follow, FollowApprovedAt, FollowId, Nanoid,
    };
    use kernel::test_utils::AccountBuilder;

    #[derive(Clone)]
    struct MockExecutor;

    impl Executor for MockExecutor {}

    struct MockDatabaseConnection;

    impl DatabaseConnection for MockDatabaseConnection {
        type Executor = MockExecutor;

        async fn get_executor(&self) -> error_stack::Result<Self::Executor, KernelError> {
            Ok(MockExecutor)
        }
    }

    struct MockAccountQueryProcessor {
        account: Account,
    }

    impl AccountQueryProcessor for MockAccountQueryProcessor {
        type Executor = MockExecutor;

        async fn find_by_id(
            &self,
            _executor: &mut Self::Executor,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok((self.account.id() == id).then(|| self.account.clone()))
        }

        async fn find_by_auth_id(
            &self,
            _executor: &mut Self::Executor,
            _auth_id: &AuthAccountId,
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(Vec::new())
        }

        async fn find_by_name(
            &self,
            _executor: &mut Self::Executor,
            name: &AccountName,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok((self.account.name() == name).then(|| self.account.clone()))
        }

        async fn find_by_nanoid(
            &self,
            _executor: &mut Self::Executor,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            Ok((self.account.nanoid() == nanoid).then(|| self.account.clone()))
        }

        async fn find_by_nanoids(
            &self,
            _executor: &mut Self::Executor,
            _nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            Ok(Vec::new())
        }

        async fn find_by_id_unfiltered(
            &self,
            executor: &mut Self::Executor,
            id: &AccountId,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_id(executor, id).await
        }

        async fn find_by_nanoid_unfiltered(
            &self,
            executor: &mut Self::Executor,
            nanoid: &Nanoid<Account>,
        ) -> error_stack::Result<Option<Account>, KernelError> {
            self.find_by_nanoid(executor, nanoid).await
        }

        async fn find_by_nanoids_unfiltered(
            &self,
            executor: &mut Self::Executor,
            nanoids: &[Nanoid<Account>],
        ) -> error_stack::Result<Vec<Account>, KernelError> {
            self.find_by_nanoids(executor, nanoids).await
        }
    }

    struct MockFollowRepository {
        followers: Vec<Follow>,
        followings: Vec<Follow>,
    }

    impl FollowRepository for MockFollowRepository {
        type Executor = MockExecutor;

        async fn find_followings(
            &self,
            _executor: &mut Self::Executor,
            _source: &FollowTargetId,
        ) -> error_stack::Result<Vec<Follow>, KernelError> {
            Ok(self.followings.clone())
        }

        async fn find_followers(
            &self,
            _executor: &mut Self::Executor,
            _destination: &FollowTargetId,
        ) -> error_stack::Result<Vec<Follow>, KernelError> {
            Ok(self.followers.clone())
        }

        async fn create(
            &self,
            _executor: &mut Self::Executor,
            _follow: &Follow,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn update(
            &self,
            _executor: &mut Self::Executor,
            _follow: &Follow,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }

        async fn delete(
            &self,
            _executor: &mut Self::Executor,
            _follow_id: &FollowId,
        ) -> error_stack::Result<(), KernelError> {
            Ok(())
        }
    }

    struct MockModule {
        database: MockDatabaseConnection,
        accounts: MockAccountQueryProcessor,
        follows: MockFollowRepository,
        public_base_url: PublicBaseUrl,
    }

    impl DependOnDatabaseConnection for MockModule {
        type DatabaseConnection = MockDatabaseConnection;

        fn database_connection(&self) -> &Self::DatabaseConnection {
            &self.database
        }
    }

    impl DependOnAccountQueryProcessor for MockModule {
        type AccountQueryProcessor = MockAccountQueryProcessor;

        fn account_query_processor(&self) -> &Self::AccountQueryProcessor {
            &self.accounts
        }
    }

    impl DependOnFollowRepository for MockModule {
        type FollowRepository = MockFollowRepository;

        fn follow_repository(&self) -> &Self::FollowRepository {
            &self.follows
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
        let account = AccountBuilder::new()
            .id(account_id.clone())
            .name("alice")
            .nanoid(Nanoid::new("alice".to_string()))
            .build();
        let approved_follower = AccountId::default();
        let pending_follower = AccountId::default();
        let approved_followee = AccountId::default();
        let pending_followee = AccountId::default();

        (
            MockModule {
                database: MockDatabaseConnection,
                accounts: MockAccountQueryProcessor { account },
                follows: MockFollowRepository {
                    followers: vec![
                        follow(approved_follower, account_id.clone(), true),
                        follow(pending_follower, account_id.clone(), false),
                    ],
                    followings: vec![
                        follow(account_id.clone(), approved_followee, true),
                        follow(account_id.clone(), pending_followee, false),
                    ],
                },
                public_base_url: PublicBaseUrl::new("https://example.com/".to_string()),
            },
            account_id,
        )
    }

    #[tokio::test]
    async fn followers_collection_returns_ordered_collection_structure() {
        let (module, account_id) = module();

        let collection = module.get_followers_collection(&account_id).await.unwrap();

        assert_eq!(
            collection.id,
            "https://example.com/ap/accounts/alice/followers"
        );
        assert_eq!(collection.type_, "OrderedCollection");
        assert_eq!(collection.total_items, Some(1));
        assert_eq!(collection.first, None);
        assert_eq!(collection.last, None);
    }

    #[tokio::test]
    async fn following_collection_returns_ordered_collection_structure() {
        let (module, account_id) = module();

        let collection = module.get_following_collection(&account_id).await.unwrap();

        assert_eq!(
            collection.id,
            "https://example.com/ap/accounts/alice/following"
        );
        assert_eq!(collection.type_, "OrderedCollection");
        assert_eq!(collection.total_items, Some(1));
        assert_eq!(collection.first, None);
        assert_eq!(collection.last, None);
    }
}
