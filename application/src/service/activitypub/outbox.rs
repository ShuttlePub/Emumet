use adapter::processor::account::{AccountQueryProcessor, DependOnAccountQueryProcessor};
use error_stack::Report;
use kernel::activitypub::{ActorUrlBuilder, OrderedCollection};
use kernel::interfaces::config::DependOnPublicBaseUrl;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::repository::{DependOnOutboxActivityRepository, OutboxActivityRepository};
use kernel::prelude::entity::{AccountId, OutboxActivity};
use kernel::KernelError;
use std::future::Future;

pub trait StoreOutboxActivityUseCase:
    'static + Sync + Send + DependOnOutboxActivityRepository
{
    fn store_outbox_activity(
        &self,
        activity: &OutboxActivity,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send {
        async move {
            let mut executor = self.database_connection().get_executor().await?;
            self.outbox_activity_repository()
                .create(&mut executor, activity)
                .await
        }
    }
}

impl<T> StoreOutboxActivityUseCase for T where
    T: 'static + Sync + Send + DependOnOutboxActivityRepository
{
}

pub trait GetOutboxUseCase:
    'static
    + Sync
    + Send
    + DependOnAccountQueryProcessor
    + DependOnOutboxActivityRepository
    + DependOnPublicBaseUrl
{
    fn get_outbox_collection(
        &self,
        account_id: &AccountId,
        limit: usize,
        cursor: Option<i64>,
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
            let activities = self
                .outbox_activity_repository()
                .find_by_account_id(&mut executor, account_id, limit, cursor)
                .await?;
            let total_items = self
                .outbox_activity_repository()
                .count_by_account_id(&mut executor, account_id)
                .await?;
            let ordered_items = activities
                .into_iter()
                .map(|activity| {
                    serde_json::from_str::<serde_json::Value>(&activity.object_json).map_err(|e| {
                        Report::new(KernelError::Internal).attach_printable(format!(
                            "Failed to deserialize outbox activity JSON: {e}"
                        ))
                    })
                })
                .collect::<error_stack::Result<Vec<_>, KernelError>>()?;

            Ok(OrderedCollection::with_ordered_items(
                ActorUrlBuilder::new(self.public_base_url().as_str(), account.nanoid().as_ref())
                    .outbox(),
                total_items,
                ordered_items,
            ))
        }
    }
}

impl<T> GetOutboxUseCase for T where
    T: 'static
        + Sync
        + Send
        + DependOnAccountQueryProcessor
        + DependOnOutboxActivityRepository
        + DependOnPublicBaseUrl
{
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::config::PublicBaseUrl;
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection, Executor};
    use kernel::prelude::entity::{Account, AccountName, AuthAccountId, Nanoid};
    use kernel::test_utils::AccountBuilder;
    use std::sync::Mutex;
    use time::OffsetDateTime;

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

    struct MockOutboxActivityRepository {
        activities: Mutex<Vec<OutboxActivity>>,
    }

    impl OutboxActivityRepository for MockOutboxActivityRepository {
        type Executor = MockExecutor;

        async fn create(
            &self,
            _executor: &mut Self::Executor,
            activity: &OutboxActivity,
        ) -> error_stack::Result<(), KernelError> {
            self.activities.lock().unwrap().push(activity.clone());
            Ok(())
        }

        async fn find_by_account_id(
            &self,
            _executor: &mut Self::Executor,
            account_id: &AccountId,
            limit: usize,
            cursor: Option<i64>,
        ) -> error_stack::Result<Vec<OutboxActivity>, KernelError> {
            let mut activities = self
                .activities
                .lock()
                .unwrap()
                .iter()
                .filter(|activity| &activity.account_id == account_id)
                .filter(|activity| cursor.map_or(true, |cursor| activity.id < cursor))
                .cloned()
                .collect::<Vec<_>>();
            activities.sort_by(|left, right| right.id.cmp(&left.id));
            activities.truncate(limit);
            Ok(activities)
        }

        async fn count_by_account_id(
            &self,
            _executor: &mut Self::Executor,
            account_id: &AccountId,
        ) -> error_stack::Result<u64, KernelError> {
            Ok(self
                .activities
                .lock()
                .unwrap()
                .iter()
                .filter(|activity| &activity.account_id == account_id)
                .count() as u64)
        }
    }

    struct MockModule {
        database: MockDatabaseConnection,
        accounts: MockAccountQueryProcessor,
        outbox: MockOutboxActivityRepository,
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

    impl DependOnOutboxActivityRepository for MockModule {
        type OutboxActivityRepository = MockOutboxActivityRepository;

        fn outbox_activity_repository(&self) -> &Self::OutboxActivityRepository {
            &self.outbox
        }
    }

    impl DependOnPublicBaseUrl for MockModule {
        fn public_base_url(&self) -> &PublicBaseUrl {
            &self.public_base_url
        }
    }

    fn module() -> (MockModule, AccountId) {
        kernel::ensure_generator_initialized();
        let account_id = AccountId::default();
        let account = AccountBuilder::new()
            .id(account_id.clone())
            .name("alice")
            .nanoid(Nanoid::new("alice".to_string()))
            .build();

        (
            MockModule {
                database: MockDatabaseConnection,
                accounts: MockAccountQueryProcessor { account },
                outbox: MockOutboxActivityRepository {
                    activities: Mutex::new(vec![outbox_activity(1, account_id.clone(), "Create")]),
                },
                public_base_url: PublicBaseUrl::new("https://example.com/".to_string()),
            },
            account_id,
        )
    }

    fn outbox_activity(id: i64, account_id: AccountId, activity_type: &str) -> OutboxActivity {
        let activity_id = format!("https://example.com/activities/{id}");
        OutboxActivity {
            id,
            account_id,
            activity_id: activity_id.clone(),
            activity_type: activity_type.to_string(),
            object_json: serde_json::json!({
                "@context": "https://www.w3.org/ns/activitystreams",
                "id": activity_id,
                "type": activity_type,
                "actor": "https://example.com/accounts/alice"
            })
            .to_string(),
            created_at: OffsetDateTime::now_utc(),
        }
    }

    #[tokio::test]
    async fn store_outbox_activity_persists_activity() {
        let (module, account_id) = module();
        let activity = outbox_activity(2, account_id.clone(), "Accept");

        module.store_outbox_activity(&activity).await.unwrap();

        let mut executor = MockExecutor;
        let activities = module
            .outbox_activity_repository()
            .find_by_account_id(&mut executor, &account_id, 10, None)
            .await
            .unwrap();
        assert!(activities.iter().any(|stored| stored.id == 2));
    }

    #[tokio::test]
    async fn outbox_collection_returns_ordered_collection_with_items() {
        let (module, account_id) = module();

        let collection = module
            .get_outbox_collection(&account_id, 10, None)
            .await
            .unwrap();

        assert_eq!(
            collection.id,
            "https://example.com/ap/accounts/alice/outbox"
        );
        assert_eq!(collection.type_, "OrderedCollection");
        assert_eq!(collection.total_items, Some(1));
        assert_eq!(collection.ordered_items.as_ref().unwrap().len(), 1);
        assert_eq!(collection.ordered_items.unwrap()[0]["type"], "Create");
    }

    #[tokio::test]
    async fn empty_outbox_collection_returns_zero_items() {
        let (module, account_id) = module();
        module.outbox.activities.lock().unwrap().clear();

        let collection = module
            .get_outbox_collection(&account_id, 10, None)
            .await
            .unwrap();

        assert_eq!(collection.total_items, Some(0));
        assert!(collection.ordered_items.unwrap().is_empty());
    }
}
