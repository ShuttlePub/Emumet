mod read_model {
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
    use kernel::interfaces::repository::{
        AuthAccountRepository, AuthHostRepository, DependOnAuthAccountRepository,
        DependOnAuthHostRepository,
    };
    use kernel::prelude::entity::{
        Account, AccountId, AuthAccountId, AuthHostId, DeletedAt, Nanoid,
    };
    use kernel::test_utils::{
        unique_account_name, AccountBuilder, AuthAccountBuilder, AuthHostBuilder,
    };
    use sqlx::types::time::OffsetDateTime;

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_by_id() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let id = AccountId::default();
        let account = AccountBuilder::new().id(id.clone()).build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();
        let result = database
            .account_read_model()
            .find_by_id(&mut conn, &id)
            .await
            .unwrap();
        assert_eq!(result.as_ref().map(Account::id), Some(account.id()));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_by_auth_id() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let accounts = database
            .account_read_model()
            .find_by_auth_id(&mut conn, &AuthAccountId::default())
            .await
            .unwrap();
        assert!(accounts.is_empty());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_by_auth_id_after_create_and_link() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new().id(host_id.clone()).build();
        database
            .auth_host_repository()
            .create(&mut conn, &auth_host)
            .await
            .unwrap();

        let auth_account_id = AuthAccountId::default();
        let auth_account = AuthAccountBuilder::new()
            .id(auth_account_id.clone())
            .host(host_id)
            .build();
        database
            .auth_account_repository()
            .create(&mut conn, &auth_account)
            .await
            .unwrap();

        let account = AccountBuilder::new().build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();

        database
            .account_read_model()
            .link_auth_account(&mut conn, account.id(), &auth_account_id)
            .await
            .unwrap();

        let accounts = database
            .account_read_model()
            .find_by_auth_id(&mut conn, &auth_account_id)
            .await
            .unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].id(), account.id());

        database
            .account_read_model()
            .unlink_all_auth_accounts(&mut conn, account.id())
            .await
            .unwrap();
        database
            .account_read_model()
            .deactivate(&mut conn, account.id())
            .await
            .unwrap();
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_auth_account_id_by_account_id_after_link_and_unlink() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let host_id = AuthHostId::default();
        let auth_host = AuthHostBuilder::new().id(host_id.clone()).build();
        database
            .auth_host_repository()
            .create(&mut conn, &auth_host)
            .await
            .unwrap();

        let auth_account_id = AuthAccountId::default();
        let auth_account = AuthAccountBuilder::new()
            .id(auth_account_id.clone())
            .host(host_id)
            .build();
        database
            .auth_account_repository()
            .create(&mut conn, &auth_account)
            .await
            .unwrap();

        let account = AccountBuilder::new().build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();

        database
            .account_read_model()
            .link_auth_account(&mut conn, account.id(), &auth_account_id)
            .await
            .unwrap();

        let linked_auth_account_id = database
            .account_read_model()
            .find_auth_account_id_by_account_id(&mut conn, account.id())
            .await
            .unwrap();
        assert_eq!(linked_auth_account_id, Some(auth_account_id));

        database
            .account_read_model()
            .unlink_all_auth_accounts(&mut conn, account.id())
            .await
            .unwrap();

        let unlinked_auth_account_id = database
            .account_read_model()
            .find_auth_account_id_by_account_id(&mut conn, account.id())
            .await
            .unwrap();
        assert_eq!(unlinked_auth_account_id, None);

        database
            .account_read_model()
            .deactivate(&mut conn, account.id())
            .await
            .unwrap();
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_by_name() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let name = unique_account_name();
        let account = AccountBuilder::new().name(name.as_ref()).build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();

        let result = database
            .account_read_model()
            .find_by_name(&mut conn, &name)
            .await
            .unwrap();
        assert_eq!(result.as_ref().map(Account::id), Some(account.id()));
        database
            .account_read_model()
            .deactivate(&mut conn, account.id())
            .await
            .unwrap();
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_by_nanoid() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let nanoid = Nanoid::default();
        let account = AccountBuilder::new().nanoid(nanoid.clone()).build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();

        let result = database
            .account_read_model()
            .find_by_nanoid(&mut conn, &nanoid)
            .await
            .unwrap();
        assert_eq!(result.as_ref().map(Account::id), Some(account.id()));
        database
            .account_read_model()
            .deactivate(&mut conn, account.id())
            .await
            .unwrap();
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn create() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let account = AccountBuilder::new().build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();
        let result = database
            .account_read_model()
            .find_by_id(&mut conn, account.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.id(), account.id());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn update() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let account = AccountBuilder::new().build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();
        let updated_account = AccountBuilder::new()
            .id(account.id().clone())
            .name(unique_account_name())
            .is_bot(true)
            .build();
        database
            .account_read_model()
            .update(&mut conn, &updated_account)
            .await
            .unwrap();
        let result = database
            .account_read_model()
            .find_by_id(&mut conn, account.id())
            .await
            .unwrap();
        assert_eq!(result.as_ref().map(Account::id), Some(updated_account.id()));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn deactivate() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut conn = database.connection().await.unwrap();

        let account = AccountBuilder::new().build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();

        database
            .account_read_model()
            .deactivate(&mut conn, account.id())
            .await
            .unwrap();
        let result = database
            .account_read_model()
            .find_by_id(&mut conn, account.id())
            .await
            .unwrap();
        assert!(result.is_none());

        // Ignore if the account is already deleted
        let account = AccountBuilder::new()
            .deleted_at(Some(DeletedAt::new(OffsetDateTime::now_utc())))
            .build();
        database
            .account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();

        database
            .account_read_model()
            .deactivate(&mut conn, account.id())
            .await
            .unwrap();
        let result = database
            .account_read_model()
            .find_by_id(&mut conn, account.id())
            .await
            .unwrap();
        assert!(result.is_none());
    }
}
