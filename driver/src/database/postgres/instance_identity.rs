use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::crypto::{EncryptedPrivateKey, SigningAlgorithm};
use kernel::interfaces::repository::{
    DependOnInstanceIdentityRepository, InstanceIdentityRepository,
};
use kernel::prelude::entity::{
    InstanceIdentity, InstanceOrigin, InstanceSigningKey, InstanceSigningKeyId,
};
use kernel::KernelError;
use sqlx::PgConnection;
use time::OffsetDateTime;

const IDENTITY_PRIMARY_KEY: &str = "instance_identity_pkey";
const ONE_ACTIVE_KEY_INDEX: &str = "instance_signing_keys_one_active";
const KEY_ID_PRIMARY_KEY: &str = "instance_signing_keys_pkey";
const KEY_ID_URI_UNIQUE: &str = "instance_signing_keys_key_id_uri_key";

#[derive(sqlx::FromRow)]
struct InstanceIdentityRow {
    public_origin: String,
    created_at: OffsetDateTime,
}

impl TryFrom<InstanceIdentityRow> for InstanceIdentity {
    type Error = Report<KernelError>;

    fn try_from(value: InstanceIdentityRow) -> Result<Self, Self::Error> {
        let origin = InstanceOrigin::parse(&value.public_origin)
            .map_err(|e| e.change_context(KernelError::Internal))?;
        if origin.as_str() != value.public_origin {
            return Err(Report::new(KernelError::Internal).attach_printable(format!(
                "Stored instance public origin is not normalized: {}",
                value.public_origin
            )));
        }
        Ok(InstanceIdentity::new(origin, value.created_at))
    }
}

#[derive(sqlx::FromRow)]
struct InstanceSigningKeyRow {
    id: String,
    revision: i64,
    algorithm: String,
    encrypted_private_key: serde_json::Value,
    public_key_pem: String,
    key_id_uri: String,
    created_at: OffsetDateTime,
    retired_at: Option<OffsetDateTime>,
    revoked_at: Option<OffsetDateTime>,
}

impl TryFrom<InstanceSigningKeyRow> for InstanceSigningKey {
    type Error = Report<KernelError>;

    fn try_from(value: InstanceSigningKeyRow) -> Result<Self, Self::Error> {
        let algorithm: SigningAlgorithm = serde_json::from_str(&format!("\"{}\"", value.algorithm))
            .map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable(format!(
                        "Invalid instance key algorithm: {}",
                        value.algorithm
                    ))
            })?;
        let encrypted_private_key: EncryptedPrivateKey =
            serde_json::from_value(value.encrypted_private_key).map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable("Failed to deserialize the instance encrypted_private_key")
            })?;
        Ok(InstanceSigningKey::new(
            InstanceSigningKeyId::parse(&value.id)?,
            value.revision,
            algorithm,
            encrypted_private_key,
            value.public_key_pem,
            value.key_id_uri,
            value.created_at,
            value.retired_at,
            value.revoked_at,
        ))
    }
}

pub struct PostgresInstanceIdentityRepository;

impl InstanceIdentityRepository for PostgresInstanceIdentityRepository {
    type Connection = PostgresConnection;

    async fn lock_provisioning(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            "SELECT pg_advisory_xact_lock(hashtextextended('emumet.instance_identity.provisioning', 0))",
        )
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn find_identity(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Option<InstanceIdentity>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, InstanceIdentityRow>(
            //language=postgresql
            r#"
            SELECT public_origin, created_at
            FROM instance_identity
            "#,
        )
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(InstanceIdentity::try_from)
        .transpose()
    }

    async fn find_signing_keys(
        &self,
        executor: &mut Self::Connection,
    ) -> error_stack::Result<Vec<InstanceSigningKey>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, InstanceSigningKeyRow>(
            //language=postgresql
            r#"
            SELECT id, revision, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, retired_at, revoked_at
            FROM instance_signing_keys
            ORDER BY created_at, id
            "#,
        )
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|rows| {
            rows.into_iter()
                .map(InstanceSigningKey::try_from)
                .collect::<Result<_, _>>()
        })
    }

    async fn create_identity(
        &self,
        executor: &mut Self::Connection,
        identity: &InstanceIdentity,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO instance_identity (public_origin, created_at)
            VALUES ($1, $2)
            "#,
        )
        .bind(identity.public_origin().as_str())
        .bind(identity.created_at)
        .execute(con)
        .await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_error))
                if db_error.constraint() == Some(IDENTITY_PRIMARY_KEY) =>
            {
                Err(Report::new(KernelError::Concurrency)
                    .attach_printable("The instance identity already exists"))
            }
            Err(e) => Err::<(), sqlx::Error>(e).convert_error(),
        }
    }

    async fn create_signing_key(
        &self,
        executor: &mut Self::Connection,
        signing_key: &InstanceSigningKey,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let encrypted_private_key_json = serde_json::to_value(signing_key.encrypted_private_key())
            .map_err(|e| {
                Report::from(e)
                    .change_context(KernelError::Internal)
                    .attach_printable("Failed to serialize the instance encrypted_private_key")
            })?;
        let result = sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO instance_signing_keys (id, revision, algorithm, encrypted_private_key, public_key_pem, key_id_uri, created_at, retired_at, revoked_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            "#,
        )
        .bind(signing_key.id().as_str())
        .bind(signing_key.revision())
        .bind(signing_key.algorithm().to_string())
        .bind(encrypted_private_key_json)
        .bind(&signing_key.public_key_pem)
        .bind(&signing_key.key_id_uri)
        .bind(signing_key.created_at)
        .bind(signing_key.retired_at)
        .bind(signing_key.revoked_at)
        .execute(con)
        .await;
        match result {
            Ok(_) => Ok(()),
            Err(sqlx::Error::Database(db_error))
                if matches!(
                    db_error.constraint(),
                    Some(ONE_ACTIVE_KEY_INDEX | KEY_ID_PRIMARY_KEY | KEY_ID_URI_UNIQUE)
                ) =>
            {
                Err(Report::new(KernelError::Concurrency).attach_printable(
                    "An active instance signing key or this key id already exists",
                ))
            }
            Err(e) => Err::<(), sqlx::Error>(e).convert_error(),
        }
    }
}

impl DependOnInstanceIdentityRepository for PostgresDatabase {
    type InstanceIdentityRepository = PostgresInstanceIdentityRepository;

    fn instance_identity_repository(&self) -> &Self::InstanceIdentityRepository {
        &PostgresInstanceIdentityRepository
    }
}
