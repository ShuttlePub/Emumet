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
}

impl DependOnProfileSigningKeyRepository for PostgresDatabase {
    type ProfileSigningKeyRepository = PostgresProfileSigningKeyRepository;

    fn profile_signing_key_repository(&self) -> &Self::ProfileSigningKeyRepository {
        &PostgresProfileSigningKeyRepository
    }
}
