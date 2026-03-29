use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{DependOnSigningKeyRepository, SigningKeyRepository};
use kernel::prelude::entity::{AccountId, SigningKey, SigningKeyId};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

#[derive(sqlx::FromRow)]
struct SigningKeyRow {
    id: i64,
    account_id: i64,
    algorithm: String,
    encrypted_private_key: serde_json::Value,
    public_key_pem: String,
    key_id_uri: String,
    created_at: OffsetDateTime,
    revoked_at: Option<OffsetDateTime>,
}

impl TryFrom<SigningKeyRow> for SigningKey {
    type Error = Report<KernelError>;

    fn try_from(value: SigningKeyRow) -> Result<Self, Self::Error> {
        let algorithm: kernel::interfaces::crypto::SigningAlgorithm =
            serde_json::from_str(&format!("\"{}\"", value.algorithm)).map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable(format!("Invalid algorithm: {}", value.algorithm))
            })?;

        let encrypted_private_key: kernel::interfaces::crypto::EncryptedPrivateKey =
            serde_json::from_value(value.encrypted_private_key).map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable("Failed to deserialize encrypted_private_key")
            })?;

        Ok(SigningKey::new(
            SigningKeyId::new(value.id),
            AccountId::new(value.account_id),
            algorithm,
            encrypted_private_key,
            value.public_key_pem,
            value.key_id_uri,
            value.created_at,
            value.revoked_at,
        ))
    }
}

pub struct PostgresSigningKeyRepository;

impl SigningKeyRepository for PostgresSigningKeyRepository {
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &SigningKeyId,
    ) -> error_stack::Result<SigningKey, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, SigningKeyRow>(
            //language=postgresql
            r#"
            SELECT id, account_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at
            FROM signing_keys
            WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("SigningKey not found: {}", id))
        })
        .and_then(SigningKey::try_from)
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<SigningKey>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, SigningKeyRow>(
            //language=postgresql
            r#"
            SELECT id, account_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at
            FROM signing_keys
            WHERE account_id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|rows| {
            rows.into_iter()
                .map(SigningKey::try_from)
                .collect::<Result<_, _>>()
        })
    }

    async fn find_active_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<Vec<SigningKey>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, SigningKeyRow>(
            //language=postgresql
            r#"
            SELECT id, account_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at
            FROM signing_keys
            WHERE account_id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|rows| {
            rows.into_iter()
                .map(SigningKey::try_from)
                .collect::<Result<_, _>>()
        })
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
        signing_key: &SigningKey,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let encrypted_private_key_json = serde_json::to_value(signing_key.encrypted_private_key())
            .map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable("Failed to serialize encrypted_private_key")
            })?;

        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO signing_keys (id, account_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(signing_key.id().as_ref())
        .bind(signing_key.account_id().as_ref())
        .bind(signing_key.algorithm().to_string())
        .bind(encrypted_private_key_json)
        .bind(&signing_key.public_key_pem)
        .bind(&signing_key.key_id_uri)
         .bind(signing_key.created_at)
         .bind(signing_key.revoked_at)
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn revoke(
        &self,
        executor: &mut Self::Executor,
        id: &SigningKeyId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE signing_keys
            SET revoked_at = NOW()
            WHERE id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable(format!("SigningKey not found or already revoked: {}", id)));
        }
        Ok(())
    }
}

impl DependOnSigningKeyRepository for PostgresDatabase {
    type SigningKeyRepository = PostgresSigningKeyRepository;

    fn signing_key_repository(&self) -> &Self::SigningKeyRepository {
        &PostgresSigningKeyRepository
    }
}

#[cfg(test)]
mod test {
    use crate::database::PostgresDatabase;
    use kernel::interfaces::crypto::{EncryptedPrivateKey, SigningAlgorithm};
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
    use kernel::interfaces::repository::{DependOnSigningKeyRepository, SigningKeyRepository};
    use kernel::prelude::entity::{AccountId, SigningKey, SigningKeyId};
    use kernel::test_utils::AccountBuilder;
    use time::OffsetDateTime;

    fn build_test_signing_key(account_id: AccountId) -> SigningKey {
        kernel::ensure_generator_initialized();
        SigningKey::new(
            SigningKeyId::default(),
            account_id,
            SigningAlgorithm::default(),
            EncryptedPrivateKey {
                ciphertext: "test-ciphertext".to_string(),
                nonce: "test-nonce".to_string(),
                salt: "test-salt".to_string(),
                algorithm: SigningAlgorithm::default(),
            },
            "-----BEGIN PUBLIC KEY-----\ntest\n-----END PUBLIC KEY-----".to_string(),
            "https://example.com/accounts/abc123#main-key".to_string(),
            OffsetDateTime::now_utc(),
            None,
        )
    }

    async fn setup_account(
        database: &PostgresDatabase,
    ) -> (AccountId, kernel::prelude::entity::Account) {
        let mut transaction = database.get_executor().await.unwrap();
        let account_id = AccountId::default();
        let account = AccountBuilder::new()
            .id(account_id.clone())
            .name(&format!("signing-key-test-{}", account_id.as_ref()))
            .build();
        database
            .account_read_model()
            .create(&mut transaction, &account)
            .await
            .unwrap();
        (account_id, account)
    }

    async fn cleanup_account(
        database: &PostgresDatabase,
        account: &kernel::prelude::entity::Account,
    ) {
        let mut transaction = database.get_executor().await.unwrap();
        database
            .account_read_model()
            .deactivate(&mut transaction, account.id())
            .await
            .unwrap();
    }

    mod query {
        use super::*;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let (account_id, account) = setup_account(&database).await;

            let signing_key = build_test_signing_key(account_id);

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &signing_key)
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            let found = database
                .signing_key_repository()
                .find_by_id(&mut executor, signing_key.id())
                .await
                .unwrap();

            assert_eq!(found.id(), signing_key.id());
            assert_eq!(found.account_id(), signing_key.account_id());
            assert_eq!(&found.public_key_pem, &signing_key.public_key_pem);
            assert_eq!(&found.key_id_uri, &signing_key.key_id_uri);

            cleanup_account(&database, &account).await;
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id_not_found() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut executor = database.get_executor().await.unwrap();

            let result = database
                .signing_key_repository()
                .find_by_id(&mut executor, &SigningKeyId::new(999999999))
                .await;

            assert!(result.is_err());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_account_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let (account_id, account) = setup_account(&database).await;

            let key1 = build_test_signing_key(account_id.clone());
            let key2 = build_test_signing_key(account_id.clone());

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &key1)
                .await
                .unwrap();
            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &key2)
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            let found = database
                .signing_key_repository()
                .find_by_account_id(&mut executor, &account_id)
                .await
                .unwrap();

            assert_eq!(found.len(), 2);

            cleanup_account(&database, &account).await;
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_active_by_account_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let (account_id, account) = setup_account(&database).await;

            let key1 = build_test_signing_key(account_id.clone());
            let key2 = build_test_signing_key(account_id.clone());

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &key1)
                .await
                .unwrap();
            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &key2)
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .revoke(&mut executor, key1.id())
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            let active = database
                .signing_key_repository()
                .find_active_by_account_id(&mut executor, &account_id)
                .await
                .unwrap();

            assert_eq!(active.len(), 1);
            assert_eq!(active[0].id(), key2.id());

            cleanup_account(&database, &account).await;
        }
    }

    mod modify {
        use super::*;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let (account_id, account) = setup_account(&database).await;

            let signing_key = build_test_signing_key(account_id);

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &signing_key)
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            let found = database
                .signing_key_repository()
                .find_by_id(&mut executor, signing_key.id())
                .await
                .unwrap();

            assert_eq!(found.id(), signing_key.id());

            cleanup_account(&database, &account).await;
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn revoke() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let (account_id, account) = setup_account(&database).await;

            let signing_key = build_test_signing_key(account_id.clone());

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &signing_key)
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .revoke(&mut executor, signing_key.id())
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            let found = database
                .signing_key_repository()
                .find_by_id(&mut executor, signing_key.id())
                .await
                .unwrap();

            assert!(found.revoked_at.is_some());

            let mut executor = database.get_executor().await.unwrap();
            let active = database
                .signing_key_repository()
                .find_active_by_account_id(&mut executor, &account_id)
                .await
                .unwrap();

            assert!(active.is_empty());

            cleanup_account(&database, &account).await;
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn revoke_not_found() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();

            let mut executor = database.get_executor().await.unwrap();
            let result = database
                .signing_key_repository()
                .revoke(&mut executor, &SigningKeyId::new(999999999))
                .await;

            assert!(result.is_err());
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn revoke_already_revoked() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let (account_id, account) = setup_account(&database).await;

            let signing_key = build_test_signing_key(account_id);

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .create(&mut executor, &signing_key)
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            database
                .signing_key_repository()
                .revoke(&mut executor, signing_key.id())
                .await
                .unwrap();

            let mut executor = database.get_executor().await.unwrap();
            let result = database
                .signing_key_repository()
                .revoke(&mut executor, signing_key.id())
                .await;

            assert!(result.is_err());

            cleanup_account(&database, &account).await;
        }
    }
}
