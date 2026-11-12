use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::crypto::{EncryptedPrivateKey, SigningAlgorithm};
use kernel::interfaces::repository::{
    DependOnProfileSigningKeyRepository, ProfileSigningKeyRepository,
};
use kernel::prelude::entity::{ProfileId, ProfileSigningKey, SigningKeyId};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

const ONE_ACTIVE_KEY_INDEX: &str = "profile_signing_keys_one_active_per_profile";

#[derive(sqlx::FromRow)]
struct ProfileSigningKeyRow {
    id: i64,
    profile_id: i64,
    algorithm: String,
    encrypted_private_key: serde_json::Value,
    public_key_pem: String,
    key_id_uri: String,
    created_at: OffsetDateTime,
    revoked_at: Option<OffsetDateTime>,
}

impl TryFrom<ProfileSigningKeyRow> for ProfileSigningKey {
    type Error = Report<KernelError>;

    fn try_from(value: ProfileSigningKeyRow) -> Result<Self, Self::Error> {
        let algorithm: SigningAlgorithm = serde_json::from_str(&format!("\"{}\"", value.algorithm))
            .map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable(format!("Invalid algorithm: {}", value.algorithm))
            })?;
        let encrypted_private_key: EncryptedPrivateKey =
            serde_json::from_value(value.encrypted_private_key).map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable("Failed to deserialize encrypted_private_key")
            })?;
        Ok(ProfileSigningKey::new(
            SigningKeyId::new(value.id),
            ProfileId::new(value.profile_id),
            algorithm,
            encrypted_private_key,
            value.public_key_pem,
            value.key_id_uri,
            value.created_at,
            value.revoked_at,
        ))
    }
}

pub struct PostgresProfileSigningKeyRepository;

impl ProfileSigningKeyRepository for PostgresProfileSigningKeyRepository {
    type Connection = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &SigningKeyId,
    ) -> error_stack::Result<ProfileSigningKey, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileSigningKeyRow>(
            //language=postgresql
            r#"
            SELECT id, profile_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at
            FROM profile_signing_keys
            WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("ProfileSigningKey not found: {}", id))
        })
        .and_then(ProfileSigningKey::try_from)
    }

    async fn find_by_profile_id(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<Vec<ProfileSigningKey>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileSigningKeyRow>(
            //language=postgresql
            r#"
            SELECT id, profile_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at
            FROM profile_signing_keys
            WHERE profile_id = $1
            ORDER BY created_at, id
            "#,
        )
        .bind(profile_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|rows| {
            rows.into_iter()
                .map(ProfileSigningKey::try_from)
                .collect::<Result<_, _>>()
        })
    }

    async fn find_active_by_profile_id(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<Vec<ProfileSigningKey>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileSigningKeyRow>(
            //language=postgresql
            r#"
            SELECT id, profile_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at
            FROM profile_signing_keys
            WHERE profile_id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(profile_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|rows| {
            rows.into_iter()
                .map(ProfileSigningKey::try_from)
                .collect::<Result<_, _>>()
        })
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
        signing_key: &ProfileSigningKey,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let encrypted_private_key_json = serde_json::to_value(signing_key.encrypted_private_key())
            .map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable("Failed to serialize encrypted_private_key")
            })?;

        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profile_signing_keys (id, profile_id, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, revoked_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(signing_key.id().as_ref())
        .bind(signing_key.profile_id().as_ref())
        .bind(signing_key.algorithm().to_string())
        .bind(encrypted_private_key_json)
        .bind(&signing_key.public_key_pem)
        .bind(&signing_key.key_id_uri)
        .bind(signing_key.created_at)
        .bind(signing_key.revoked_at)
        .execute(con)
        .await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_error))
                if db_error.constraint() == Some(ONE_ACTIVE_KEY_INDEX) =>
            {
                Err(Report::new(KernelError::Concurrency)
                    .attach_printable("Profile already has an active signing key"))
            }
            Err(e) => Err::<(), sqlx::Error>(e).convert_error(),
        }
    }

    async fn revoke(
        &self,
        executor: &mut Self::Connection,
        id: &SigningKeyId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE profile_signing_keys
            SET revoked_at = NOW()
            WHERE id = $1 AND revoked_at IS NULL
            "#,
        )
        .bind(id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound).attach_printable(format!(
                "ProfileSigningKey not found or already revoked: {}",
                id
            )));
        }
        Ok(())
    }

    async fn lock_provisioning(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('emumet.profile_signing_keys.provisioning', 0))")
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }

    async fn find_profiles_without_active_key(
        &self,
        executor: &mut Self::Connection,
        limit: i64,
    ) -> error_stack::Result<Vec<(ProfileId, String)>, KernelError> {
        let con: &mut PgConnection = executor;
        let rows: Vec<(i64, String)> = sqlx::query_as(
            //language=postgresql
            r#"
            SELECT p.id, p.nanoid
            FROM profiles p
            WHERE NOT EXISTS (
                SELECT 1 FROM profile_signing_keys k
                WHERE k.profile_id = p.id AND k.revoked_at IS NULL
            )
            ORDER BY p.id
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(con)
        .await
        .convert_error()?;
        Ok(rows
            .into_iter()
            .map(|(id, nanoid)| (ProfileId::new(id), nanoid))
            .collect())
    }
}

impl DependOnProfileSigningKeyRepository for PostgresDatabase {
    type ProfileSigningKeyRepository = PostgresProfileSigningKeyRepository;

    fn profile_signing_key_repository(&self) -> &Self::ProfileSigningKeyRepository {
        &PostgresProfileSigningKeyRepository
    }
}

#[cfg(test)]
mod test {
    use crate::crypto::{Argon2Encryptor, Rsa2048RawGenerator};
    use crate::database::PostgresDatabase;
    use crate::http_signing::{HttpSignatureVerifierImpl, HttpSignerImpl};
    use base64::{engine::general_purpose, Engine as _};
    use kernel::activitypub::ActorUrlBuilder;
    use kernel::interfaces::crypto::{
        EncryptedPrivateKey, KeyEncryptor, RawKeyGenerator, SigningAlgorithm,
    };
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::http_signing::{
        HttpSignatureVerificationInput, HttpSignatureVerifier, HttpSigner, HttpSigningRequest,
        SignatureVerificationResult,
    };
    use kernel::interfaces::read_model::{
        AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
    };
    use kernel::interfaces::repository::{
        DependOnProfileSigningKeyRepository, ProfileSigningKeyRepository,
    };
    use kernel::prelude::entity::{AccountId, ProfileId, ProfileSigningKey, SigningKeyId};
    use kernel::test_utils::{AccountBuilder, ProfileBuilder};
    use kernel::KernelError;
    use sha2::Digest;
    use std::collections::HashMap;
    use time::OffsetDateTime;

    async fn seed_profile(db: &PostgresDatabase) -> (ProfileId, String) {
        kernel::ensure_generator_initialized();
        let mut conn = db.connection().await.unwrap();
        let account_id = AccountId::default();
        let account = AccountBuilder::new()
            .id(account_id.clone())
            .name(format!("profile-key-test-{}", account_id.as_ref()))
            .build();
        db.account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();
        let profile = ProfileBuilder::new().account_id(account_id).build();
        db.profile_read_model()
            .create(&mut conn, &profile)
            .await
            .unwrap();
        (profile.id().clone(), profile.nanoid().as_ref().to_string())
    }

    fn key_for(profile_id: &ProfileId, nanoid: &str) -> ProfileSigningKey {
        ProfileSigningKey::new(
            SigningKeyId::default(),
            profile_id.clone(),
            SigningAlgorithm::Rsa2048,
            EncryptedPrivateKey {
                ciphertext: "c".to_string(),
                nonce: "n".to_string(),
                salt: "s".to_string(),
                algorithm: SigningAlgorithm::Rsa2048,
            },
            "-----BEGIN PUBLIC KEY-----\ntest\n-----END PUBLIC KEY-----".to_string(),
            ActorUrlBuilder::for_profile("https://example.com", nanoid).key_id(),
            OffsetDateTime::now_utc(),
            None,
        )
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn one_active_key_per_profile_and_revocation_allows_rotation() {
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let repository = db.profile_signing_key_repository();
        let (profile_id, nanoid) = seed_profile(&db).await;

        let first = key_for(&profile_id, &nanoid);
        repository.create(&mut conn, &first).await.unwrap();
        let loaded = repository.find_by_id(&mut conn, first.id()).await.unwrap();
        assert_eq!(loaded.profile_id(), &profile_id);
        assert_eq!(loaded.key_id_uri, first.key_id_uri);
        assert_eq!(loaded.public_key_pem, first.public_key_pem);
        assert_eq!(
            repository
                .find_active_by_profile_id(&mut conn, &profile_id)
                .await
                .unwrap()
                .len(),
            1
        );

        let second = key_for(&profile_id, &nanoid);
        let error = repository.create(&mut conn, &second).await.unwrap_err();
        assert_eq!(error.current_context(), &KernelError::Concurrency);

        repository.revoke(&mut conn, first.id()).await.unwrap();
        assert!(repository
            .find_active_by_profile_id(&mut conn, &profile_id)
            .await
            .unwrap()
            .is_empty());
        repository.create(&mut conn, &second).await.unwrap();
        assert_eq!(
            repository
                .find_by_profile_id(&mut conn, &profile_id)
                .await
                .unwrap()
                .len(),
            2
        );
        let error = repository.revoke(&mut conn, first.id()).await.unwrap_err();
        assert_eq!(error.current_context(), &KernelError::NotFound);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn keys_survive_the_deletion_of_the_profile_row_and_reattach() {
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let repository = db.profile_signing_key_repository();
        let (profile_id, nanoid) = seed_profile(&db).await;
        let key = key_for(&profile_id, &nanoid);
        repository.create(&mut conn, &key).await.unwrap();
        // Deactivating the Account deletes the Profile projection row.
        let profile = db
            .profile_read_model()
            .find_by_id_unfiltered(&mut conn, &profile_id)
            .await
            .unwrap()
            .unwrap();
        db.profile_read_model()
            .delete(&mut conn, &profile_id)
            .await
            .unwrap();
        let retained = repository
            .find_active_by_profile_id(&mut conn, &profile_id)
            .await
            .unwrap();
        assert_eq!(retained.len(), 1, "the key must survive the Profile row");
        assert_eq!(retained[0].id(), key.id());
        // The projector re-materializes the same Profile id: the key is attached again.
        db.profile_read_model()
            .create(&mut conn, &kernel::prelude::entity::Profile::from(profile))
            .await
            .unwrap();
        assert_eq!(
            repository
                .find_active_by_profile_id(&mut conn, &profile_id)
                .await
                .unwrap()[0]
                .public_key_pem,
            key.public_key_pem
        );
        // A second active key is still refused (one active key per Profile).
        let error = repository
            .create(&mut conn, &key_for(&profile_id, &nanoid))
            .await
            .unwrap_err();
        assert_eq!(error.current_context(), &KernelError::Concurrency);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn a_signature_made_with_the_stored_profile_key_verifies() {
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let repository = db.profile_signing_key_repository();
        let (profile_id, nanoid) = seed_profile(&db).await;

        // Generate and store exactly like the Profile key use case does.
        let password = b"profile-key-test-password";
        let raw = Rsa2048RawGenerator.generate_raw().unwrap();
        let encrypted = Argon2Encryptor::default()
            .encrypt(&raw.private_key_pem, password, raw.algorithm)
            .unwrap();
        let key = ProfileSigningKey::new(
            SigningKeyId::default(),
            profile_id.clone(),
            raw.algorithm,
            encrypted,
            raw.public_key_pem.clone(),
            ActorUrlBuilder::for_profile("https://example.com", &nanoid).key_id(),
            OffsetDateTime::now_utc(),
            None,
        );
        repository.create(&mut conn, &key).await.unwrap();

        // Load, decrypt and sign with the stored key.
        let stored = repository
            .find_active_by_profile_id(&mut conn, &profile_id)
            .await
            .unwrap()
            .remove(0);
        let private_key_pem = Argon2Encryptor::default()
            .decrypt(stored.encrypted_private_key(), password)
            .unwrap();
        let body = b"{\"type\":\"Follow\"}";
        let mut headers = HashMap::new();
        headers.insert("host".to_string(), "remote.example".to_string());
        headers.insert(
            "date".to_string(),
            httpdate::fmt_http_date(std::time::SystemTime::now()),
        );
        headers.insert(
            "digest".to_string(),
            format!(
                "SHA-256={}",
                general_purpose::STANDARD.encode(sha2::Sha256::digest(body))
            ),
        );
        let request = HttpSigningRequest {
            method: "POST".to_string(),
            url: "https://remote.example/inbox".to_string(),
            headers,
            body: Some(body.to_vec()),
        };
        let signed = HttpSignerImpl
            .sign(
                &request,
                &private_key_pem,
                &stored.key_id_uri,
                stored.algorithm(),
            )
            .await
            .unwrap();

        let mut verifier = HttpSignatureVerifierImpl::new().unwrap();
        verifier.cache_actor_key(&stored.key_id_uri, stored.public_key_pem.clone());
        let input = HttpSignatureVerificationInput {
            method: request.method.clone(),
            url: request.url.clone(),
            headers: signed.cavage_headers.clone(),
            body: request.body.clone(),
        };
        assert_eq!(
            verifier.verify(&input).await.unwrap(),
            SignatureVerificationResult::Valid {
                key_id: stored.key_id_uri.clone()
            }
        );

        // A tampered body no longer verifies.
        let mut tampered = input;
        tampered.body = Some(b"{\"type\":\"Undo\"}".to_vec());
        assert!(matches!(
            verifier.verify(&tampered).await.unwrap(),
            SignatureVerificationResult::Invalid(_)
        ));
    }
}
