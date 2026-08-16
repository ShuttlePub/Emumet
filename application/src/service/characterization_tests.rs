use super::account::{
    BanAccountUseCase, CreateAccountUseCase, DeactivateAccountUseCase, SuspendAccountUseCase,
    UnsuspendAccountUseCase,
};
use super::account_detail::UpdateAccountDetailUseCase;
use super::activitypub::inject_test_remote_actor;
use super::block::{BlockAccountUseCase, UnblockAccountUseCase};
use super::mute::{MuteAccountUseCase, UnmuteAccountUseCase};
use crate::dto::account::{AccountFieldDto, CreateAccountDto, UpdateAccountDto};
use crate::dto::block_mute::{BlockAccountDto, MuteAccountDto};
use crate::projection::{ProjectMetadataBatch, ProjectProfileBatch};
use driver::crypto::{Argon2Encryptor, FilePasswordProvider, Rsa2048RawGenerator};
use driver::database::PostgresDatabase;
use driver::http_signing::HttpSignerImpl;
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnRawKeyGenerator,
};
use kernel::interfaces::database::{
    DatabaseConnection, Transaction as DbTransaction, TransactionManager,
    TransactionalDatabaseConnection,
};
use kernel::interfaces::http_signing::DependOnHttpSigner;
use kernel::interfaces::permission::{
    DependOnPermissionChecker, DependOnPermissionWriter, InstanceRole, PermissionChecker,
    PermissionReq, PermissionWriter, RelationTarget,
};
use kernel::interfaces::repository::{
    DependOnMuteRepository, DependOnOutboxActivityRepository, MuteRepository,
    OutboxActivityRepository,
};
use kernel::prelude::entity::{AccountId, AuthAccountId, FieldAction, MuteTargetId};
use kernel::KernelError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

struct AllowPermissions {
    writes: AtomicUsize,
    deletes: AtomicUsize,
    database: PostgresDatabase,
    committed_write_observations: Mutex<Vec<(i64, i64, i64)>>,
}

impl AllowPermissions {
    fn new(database: PostgresDatabase) -> Self {
        Self {
            writes: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            database,
            committed_write_observations: Mutex::new(Vec::new()),
        }
    }
}

impl PermissionChecker for AllowPermissions {
    async fn check(
        &self,
        _subject: &AuthAccountId,
        _req: &PermissionReq,
    ) -> error_stack::Result<bool, KernelError> {
        Ok(true)
    }

    async fn list_instance_roles(
        &self,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
        Ok(Vec::new())
    }
}

impl PermissionWriter for AllowPermissions {
    async fn create_relation(
        &self,
        target: &RelationTarget,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        match target {
            RelationTarget::Account { account_id, .. } => {
                let observed: (i64, i64, i64) = sqlx::query_as(
                    "SELECT (SELECT COUNT(*) FROM account_events WHERE id = $1), \
                     (SELECT COUNT(*) FROM profile_events WHERE (data->>'account_id')::bigint = $1), \
                     (SELECT COUNT(*) FROM signing_keys WHERE account_id = $1)",
                )
                .bind(account_id.as_ref())
                .fetch_one(&mut *self.database.connection().await?)
                .await
                .map_err(|error| {
                    error_stack::Report::from(error).change_context(KernelError::Internal)
                })?;
                self.committed_write_observations
                    .lock()
                    .unwrap()
                    .push(observed);
            }
            RelationTarget::Instance { .. } => {}
        }
        Ok(())
    }

    async fn delete_relation(
        &self,
        _target: &RelationTarget,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.deletes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Clone)]
struct TestModule {
    database: PostgresDatabase,
    password_provider: std::sync::Arc<FilePasswordProvider>,
    raw_key_generator: std::sync::Arc<Rsa2048RawGenerator>,
    key_encryptor: std::sync::Arc<Argon2Encryptor>,
    permissions: std::sync::Arc<AllowPermissions>,
    public_base_url: PublicBaseUrl,
    http_signer: HttpSignerImpl,
}

impl TestModule {
    async fn new(password_path: &std::path::Path) -> Self {
        let database = PostgresDatabase::new().await.unwrap();
        Self {
            permissions: std::sync::Arc::new(AllowPermissions::new(database.clone())),
            database,
            password_provider: std::sync::Arc::new(FilePasswordProvider::with_paths(
                password_path,
                password_path,
            )),
            raw_key_generator: std::sync::Arc::new(Rsa2048RawGenerator),
            key_encryptor: std::sync::Arc::new(Argon2Encryptor::default()),
            public_base_url: PublicBaseUrl::new("https://example.com".to_string()),
            http_signer: HttpSignerImpl,
        }
    }

    async fn seed_auth_account(&self, auth_account_id: &AuthAccountId) {
        let mut conn = self.database.connection().await.unwrap();
        let host_id = kernel::generate_id();
        sqlx::query("INSERT INTO auth_hosts (id, url) VALUES ($1, $2)")
            .bind(host_id)
            .bind(format!("https://auth-{host_id}.example.com"))
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO auth_accounts (id, host_id, client_id) VALUES ($1, $2, $3)")
            .bind(auth_account_id.as_ref())
            .bind(host_id)
            .bind(format!("client-{host_id}"))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
}

kernel::impl_database_delegation!(TestModule, database, PostgresDatabase);

impl DependOnPasswordProvider for TestModule {
    type PasswordProvider = FilePasswordProvider;

    fn password_provider(&self) -> &Self::PasswordProvider {
        self.password_provider.as_ref()
    }
}

impl DependOnRawKeyGenerator for TestModule {
    type RawKeyGenerator = Rsa2048RawGenerator;

    fn raw_key_generator(&self) -> &Self::RawKeyGenerator {
        self.raw_key_generator.as_ref()
    }
}

impl DependOnKeyEncryptor for TestModule {
    type KeyEncryptor = Argon2Encryptor;

    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        self.key_encryptor.as_ref()
    }
}

impl DependOnPermissionChecker for TestModule {
    type PermissionChecker = AllowPermissions;

    fn permission_checker(&self) -> &Self::PermissionChecker {
        self.permissions.as_ref()
    }
}

impl DependOnPermissionWriter for TestModule {
    type PermissionWriter = AllowPermissions;

    fn permission_writer(&self) -> &Self::PermissionWriter {
        self.permissions.as_ref()
    }
}

impl DependOnPublicBaseUrl for TestModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.public_base_url
    }
}

impl DependOnHttpSigner for TestModule {
    type HttpSigner = HttpSignerImpl;

    fn http_signer(&self) -> &Self::HttpSigner {
        &self.http_signer
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_account_persists_current_four_write_orchestration() {
    // Given
    kernel::ensure_generator_initialized();
    let password_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password_file.path(), b"characterization-password").unwrap();
    let module = TestModule::new(password_file.path()).await;
    let auth_account_id = AuthAccountId::default();
    module.seed_auth_account(&auth_account_id).await;
    let name = kernel::test_utils::unique_account_name()
        .as_ref()
        .to_string();

    // When
    let created = module
        .create_account(
            auth_account_id,
            CreateAccountDto {
                name: name.clone(),
                is_bot: false,
            },
        )
        .await
        .unwrap();

    // Then
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM account_events WHERE data->>'name' = $1), \
         (SELECT COUNT(*) FROM profile_events p JOIN accounts a ON a.id = (p.data->>'account_id')::bigint WHERE a.name = $1), \
         (SELECT COUNT(*) FROM signing_keys s JOIN accounts a ON a.id = s.account_id WHERE a.name = $1)",
    )
    .bind(&name)
    .fetch_one(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert_eq!(created.name, name);
    assert_eq!(row, (1, 1, 1));
    assert_eq!(module.permissions.writes.load(Ordering::Relaxed), 1);
    assert_eq!(
        module
            .permissions
            .committed_write_observations
            .lock()
            .unwrap()
            .as_slice(),
        &[(1, 1, 1)],
        "Keto provisioning must observe all database writes after commit"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn create_account_rolls_back_all_database_writes_when_signing_key_creation_fails() {
    // Given
    kernel::ensure_generator_initialized();
    let temp_dir = tempfile::tempdir().unwrap();
    let missing_password = temp_dir.path().join("missing-password");
    let module = TestModule::new(&missing_password).await;
    let auth_account_id = AuthAccountId::default();
    module.seed_auth_account(&auth_account_id).await;
    let name = kernel::test_utils::unique_account_name()
        .as_ref()
        .to_string();

    // When
    let result = module
        .create_account(
            auth_account_id.clone(),
            CreateAccountDto {
                name: name.clone(),
                is_bot: false,
            },
        )
        .await;

    // Then
    assert!(result.is_err());
    let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM accounts WHERE name = $1), \
         (SELECT COUNT(*) FROM account_events WHERE data->>'name' = $1), \
         (SELECT COUNT(*) FROM profiles p JOIN accounts a ON a.id = p.account_id WHERE a.name = $1), \
         (SELECT COUNT(*) FROM profile_events p JOIN accounts a ON a.id = (p.data->>'account_id')::bigint WHERE a.name = $1), \
         (SELECT COUNT(*) FROM signing_keys s JOIN accounts a ON a.id = s.account_id WHERE a.name = $1), \
         (SELECT COUNT(*) FROM auth_accounts WHERE id = $2)",
    )
    .bind(&name)
    .bind(auth_account_id.as_ref())
    .fetch_one(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert_eq!(counts, (0, 0, 0, 0, 0, 1));
    assert_eq!(module.permissions.writes.load(Ordering::Relaxed), 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn update_account_detail_commits_all_current_database_writes() {
    // Given
    kernel::ensure_generator_initialized();
    let password_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password_file.path(), b"characterization-password").unwrap();
    let module = TestModule::new(password_file.path()).await;
    let auth_account_id = AuthAccountId::default();
    module.seed_auth_account(&auth_account_id).await;
    let created = module
        .create_account(
            auth_account_id.clone(),
            CreateAccountDto {
                name: kernel::test_utils::unique_account_name()
                    .as_ref()
                    .to_string(),
                is_bot: false,
            },
        )
        .await
        .unwrap();

    // When
    let updated = module
        .update_account_detail(
            &auth_account_id,
            UpdateAccountDto {
                account_nanoid: created.nanoid.clone(),
                is_bot: FieldAction::Set(true),
                display_name: FieldAction::Set("Updated display name".to_string()),
                summary: FieldAction::Set("Updated summary".to_string()),
                icon_url: FieldAction::Unchanged,
                banner_url: FieldAction::Unchanged,
                fields: Some(vec![AccountFieldDto {
                    label: "site".to_string(),
                    content: "https://example.com".to_string(),
                }]),
            },
        )
        .await
        .unwrap();

    // Tailing projectors apply the profile and metadata events to the read models.
    module.database.project_profile_batch().await.unwrap();
    module.database.project_metadata_batch().await.unwrap();

    // Then
    assert!(updated.is_bot);
    assert_eq!(
        updated.display_name.as_deref(),
        Some("Updated display name")
    );
    assert_eq!(updated.summary.as_deref(), Some("Updated summary"));
    assert_eq!(
        updated.fields,
        vec![AccountFieldDto {
            label: "site".to_string(),
            content: "https://example.com".to_string(),
        }]
    );
    let persisted: (bool, Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT a.is_bot, p.display, p.summary, \
         (SELECT COUNT(*) FROM metadatas m WHERE m.account_id = a.id) \
         FROM accounts a JOIN profiles p ON p.account_id = a.id WHERE a.nanoid = $1",
    )
    .bind(&created.nanoid)
    .fetch_one(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert_eq!(
        persisted,
        (
            true,
            Some("Updated display name".to_string()),
            Some("Updated summary".to_string()),
            1
        )
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn update_account_detail_rolls_back_account_changes_when_later_write_fails() {
    // Given
    kernel::ensure_generator_initialized();
    let password_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password_file.path(), b"characterization-password").unwrap();
    let module = TestModule::new(password_file.path()).await;
    let auth_account_id = AuthAccountId::default();
    module.seed_auth_account(&auth_account_id).await;
    let created = module
        .create_account(
            auth_account_id.clone(),
            CreateAccountDto {
                name: kernel::test_utils::unique_account_name()
                    .as_ref()
                    .to_string(),
                is_bot: false,
            },
        )
        .await
        .unwrap();

    // When
    let result = module
        .update_account_detail(
            &auth_account_id,
            UpdateAccountDto {
                account_nanoid: created.nanoid.clone(),
                is_bot: FieldAction::Set(true),
                display_name: FieldAction::Unchanged,
                summary: FieldAction::Unchanged,
                icon_url: FieldAction::Set("https://example.com/missing.png".to_string()),
                banner_url: FieldAction::Unchanged,
                fields: None,
            },
        )
        .await;

    // Then
    assert!(result.is_err());
    let persisted: (bool, i64) = sqlx::query_as(
        "SELECT a.is_bot, (SELECT COUNT(*) FROM account_events e WHERE e.id = a.id) \
         FROM accounts a WHERE a.nanoid = $1",
    )
    .bind(&created.nanoid)
    .fetch_one(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert_eq!(persisted, (false, 1));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn moderation_use_cases_preserve_current_event_sequence_and_post_commit_deprovisioning() {
    // Given
    kernel::ensure_generator_initialized();
    let password_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password_file.path(), b"characterization-password").unwrap();
    let module = TestModule::new(password_file.path()).await;
    let auth_account_id = AuthAccountId::default();
    module.seed_auth_account(&auth_account_id).await;
    let created = module
        .create_account(
            auth_account_id.clone(),
            CreateAccountDto {
                name: kernel::test_utils::unique_account_name()
                    .as_ref()
                    .to_string(),
                is_bot: false,
            },
        )
        .await
        .unwrap();

    // When
    module
        .suspend_account(
            &auth_account_id,
            created.nanoid.clone(),
            "spam".to_string(),
            None,
        )
        .await
        .unwrap();
    module
        .unsuspend_account(&auth_account_id, created.nanoid.clone())
        .await
        .unwrap();
    module
        .ban_account(
            &auth_account_id,
            created.nanoid.clone(),
            "abuse".to_string(),
        )
        .await
        .unwrap();
    module
        .deactivate_account(&auth_account_id, created.nanoid.clone())
        .await
        .unwrap();

    // Then
    let event_names: Vec<(String,)> = sqlx::query_as(
        "SELECT event_name FROM account_events e JOIN accounts a ON a.id = e.id \
         WHERE a.nanoid = $1 ORDER BY e.version",
    )
    .bind(&created.nanoid)
    .fetch_all(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert_eq!(
        event_names,
        vec![
            ("account_created".to_string(),),
            ("account_suspended".to_string(),),
            ("account_unsuspended".to_string(),),
            ("account_banned".to_string(),),
            ("account_deactivated".to_string(),),
        ]
    );
    assert_eq!(module.permissions.deletes.load(Ordering::Relaxed), 3);
}

async fn mute_test_module() -> (TestModule, tempfile::NamedTempFile, AuthAccountId) {
    kernel::ensure_generator_initialized();
    let password_file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(password_file.path(), b"characterization-password").unwrap();
    let module = TestModule::new(password_file.path()).await;
    let auth_account_id = AuthAccountId::default();
    module.seed_auth_account(&auth_account_id).await;
    (module, password_file, auth_account_id)
}

async fn create_test_account(module: &TestModule, auth_account_id: &AuthAccountId) -> String {
    module
        .create_account(
            auth_account_id.clone(),
            CreateAccountDto {
                name: kernel::test_utils::unique_account_name()
                    .as_ref()
                    .to_string(),
                is_bot: false,
            },
        )
        .await
        .unwrap()
        .nanoid
}

async fn find_local_mutes(
    module: &TestModule,
    muter_nanoid: &str,
) -> Vec<kernel::prelude::entity::Mute> {
    let mut executor = module.database.connection().await.unwrap();
    let account_id: i64 = sqlx::query_scalar("SELECT id FROM accounts WHERE nanoid = $1")
        .bind(muter_nanoid)
        .fetch_one(&mut *executor)
        .await
        .unwrap();
    module
        .mute_repository()
        .find_mutes(
            &mut executor,
            &MuteTargetId::from(AccountId::new(account_id)),
        )
        .await
        .unwrap()
}

async fn outbox_activity_count(module: &TestModule, account_nanoid: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM outbox_activities o JOIN accounts a ON a.id = o.account_id \
         WHERE a.nanoid = $1",
    )
    .bind(account_nanoid)
    .fetch_one(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn mute_use_case_equivalence_preserves_state_effects() {
    // Given
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let muter = create_test_account(&module, &auth_account_id).await;
    let target = create_test_account(&module, &auth_account_id).await;

    // When
    let relation = module
        .mute_account(
            auth_account_id,
            MuteAccountDto {
                account_nanoid: muter.clone(),
                target: target.clone(),
            },
        )
        .await
        .unwrap();

    // Then
    assert_eq!(relation.target_type, "local");
    assert_eq!(relation.target, target);
    let mutes = find_local_mutes(&module, &muter).await;
    assert_eq!(mutes.len(), 1);
    assert_eq!(relation.id, mutes[0].id().as_ref().to_string());
    assert_eq!(outbox_activity_count(&module, &muter).await, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn mute_twice_is_ok_no_rejected() {
    // Given
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let muter = create_test_account(&module, &auth_account_id).await;
    let target = create_test_account(&module, &auth_account_id).await;
    let dto = || MuteAccountDto {
        account_nanoid: muter.clone(),
        target: target.clone(),
    };
    module
        .mute_account(auth_account_id.clone(), dto())
        .await
        .unwrap();

    // When
    let second = module.mute_account(auth_account_id, dto()).await;

    // Then
    assert!(second.is_ok());
    assert_eq!(find_local_mutes(&module, &muter).await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unmute_missing_is_ok_no_not_found() {
    // Given
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let muter = create_test_account(&module, &auth_account_id).await;
    let target = create_test_account(&module, &auth_account_id).await;

    // When
    let result = module
        .unmute_account(
            auth_account_id,
            MuteAccountDto {
                account_nanoid: muter,
                target,
            },
        )
        .await;

    // Then
    assert!(result.is_ok());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unmute_removes_mute() {
    // Given
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let muter = create_test_account(&module, &auth_account_id).await;
    let target = create_test_account(&module, &auth_account_id).await;
    let dto = || MuteAccountDto {
        account_nanoid: muter.clone(),
        target: target.clone(),
    };
    module
        .mute_account(auth_account_id.clone(), dto())
        .await
        .unwrap();
    assert_eq!(find_local_mutes(&module, &muter).await.len(), 1);

    // When
    module.unmute_account(auth_account_id, dto()).await.unwrap();

    // Then
    assert_eq!(find_local_mutes(&module, &muter).await.len(), 0);
}

/// Wraps a real [`PostgresDatabase`] and forces every transaction to roll back
/// after the user closure has run, so tests can observe all-or-nothing writes.
#[derive(Clone)]
struct FaultInjectingDatabase {
    inner: PostgresDatabase,
}

impl DatabaseConnection for FaultInjectingDatabase {
    type Connection = <PostgresDatabase as DatabaseConnection>::Connection;

    async fn connection(&self) -> error_stack::Result<Self::Connection, KernelError> {
        self.inner.connection().await
    }
}

impl TransactionManager for FaultInjectingDatabase {
    fn transaction<'a, F, T>(
        &'a self,
        operation: F,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = error_stack::Result<T, KernelError>> + Send + 'a>,
    >
    where
        F: for<'connection> FnOnce(
                &'connection mut Self::Connection,
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = error_stack::Result<T, KernelError>>
                        + Send
                        + 'connection,
                >,
            > + Send
            + 'a,
        T: Send + 'a,
    {
        Box::pin(async move {
            let mut transaction = self.inner.get_transaction().await?;
            operation(transaction.connection()).await?;
            let con: &mut sqlx::PgConnection = transaction.connection();
            sqlx::query("ROLLBACK")
                .execute(&mut *con)
                .await
                .map_err(|error| {
                    error_stack::Report::new(KernelError::Internal)
                        .attach_printable(format!("injected rollback failed: {error}"))
                })?;
            Err(error_stack::Report::new(KernelError::Internal)
                .attach_printable("injected failure"))
        })
    }
}

/// Delegates every database dependence of [`FaultInjectingDatabase`] to the
/// inner `PostgresDatabase` (connection/transaction traits above are the only
/// behavioral overrides; the blanket impls wire `DependOnDatabaseConnection`
/// and `DependOnTransactionManager` with `TransactionManager = Self`).
macro_rules! delegate_database_dependence {
    ($($depend:path { $assoc:ident, $method:ident }),+ $(,)?) => {
        $(
            impl $depend for FaultInjectingDatabase {
                type $assoc = <PostgresDatabase as $depend>::$assoc;

                fn $method(&self) -> &Self::$assoc {
                    <PostgresDatabase as $depend>::$method(&self.inner)
                }
            }
        )+
    };
}

delegate_database_dependence! {
    kernel::interfaces::read_model::DependOnAccountReadModel { AccountReadModel, account_read_model },
    kernel::interfaces::read_model::DependOnProfileReadModel { ProfileReadModel, profile_read_model },
    kernel::interfaces::read_model::DependOnMetadataReadModel { MetadataReadModel, metadata_read_model },
    kernel::interfaces::event_store::DependOnAccountEventStore { AccountEventStore, account_event_store },
    kernel::interfaces::event_store::DependOnProfileEventStore { ProfileEventStore, profile_event_store },
    kernel::interfaces::event_store::DependOnMetadataEventStore { MetadataEventStore, metadata_event_store },
    kernel::interfaces::repository::DependOnAccountRepository { AccountRepository, account_repository },
    kernel::interfaces::repository::DependOnAuthAccountRepository { AuthAccountRepository, auth_account_repository },
    kernel::interfaces::repository::DependOnAuthHostRepository { AuthHostRepository, auth_host_repository },
    kernel::interfaces::repository::DependOnBlockRepository { BlockRepository, block_repository },
    kernel::interfaces::repository::DependOnFollowRepository { FollowRepository, follow_repository },
    kernel::interfaces::repository::DependOnImageRepository { ImageRepository, image_repository },
    kernel::interfaces::repository::DependOnMetadataRepository { MetadataRepository, metadata_repository },
    kernel::interfaces::repository::DependOnMuteRepository { MuteRepository, mute_repository },
    kernel::interfaces::repository::DependOnOutboxActivityRepository { OutboxActivityRepository, outbox_activity_repository },
    kernel::interfaces::repository::DependOnProfileRepository { ProfileRepository, profile_repository },
    kernel::interfaces::repository::DependOnRemoteAccountRepository { RemoteAccountRepository, remote_account_repository },
    kernel::interfaces::repository::DependOnSigningKeyRepository { SigningKeyRepository, signing_key_repository },
    kernel::interfaces::projection::DependOnAccountEventLog { AccountEventLog, account_event_log },
    kernel::interfaces::projection::DependOnAccountProjectionWriter { AccountProjectionWriter, account_projection_writer },
    kernel::interfaces::projection::DependOnMetadataEventLog { MetadataEventLog, metadata_event_log },
    kernel::interfaces::projection::DependOnMetadataProjectionWriter { MetadataProjectionWriter, metadata_projection_writer },
    kernel::interfaces::projection::DependOnProfileEventLog { ProfileEventLog, profile_event_log },
    kernel::interfaces::projection::DependOnProfileProjectionWriter { ProfileProjectionWriter, profile_projection_writer },
    kernel::interfaces::projection::DependOnProjectionCheckpointStore { ProjectionCheckpointStore, projection_checkpoint_store },
}

#[derive(Clone)]
struct TestFaultModule {
    database: FaultInjectingDatabase,
    password_provider: std::sync::Arc<FilePasswordProvider>,
    key_encryptor: std::sync::Arc<Argon2Encryptor>,
    permissions: std::sync::Arc<AllowPermissions>,
    public_base_url: PublicBaseUrl,
    http_signer: HttpSignerImpl,
}

impl TestFaultModule {
    async fn new(password_path: &std::path::Path) -> Self {
        let database = PostgresDatabase::new().await.unwrap();
        Self {
            permissions: std::sync::Arc::new(AllowPermissions::new(database.clone())),
            database: FaultInjectingDatabase { inner: database },
            password_provider: std::sync::Arc::new(FilePasswordProvider::with_paths(
                password_path,
                password_path,
            )),
            key_encryptor: std::sync::Arc::new(Argon2Encryptor::default()),
            public_base_url: PublicBaseUrl::new("https://example.com".to_string()),
            http_signer: HttpSignerImpl,
        }
    }
}

kernel::impl_database_delegation!(TestFaultModule, database, FaultInjectingDatabase);

impl DependOnPasswordProvider for TestFaultModule {
    type PasswordProvider = FilePasswordProvider;

    fn password_provider(&self) -> &Self::PasswordProvider {
        self.password_provider.as_ref()
    }
}

impl DependOnKeyEncryptor for TestFaultModule {
    type KeyEncryptor = Argon2Encryptor;

    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        self.key_encryptor.as_ref()
    }
}

impl DependOnPermissionChecker for TestFaultModule {
    type PermissionChecker = AllowPermissions;

    fn permission_checker(&self) -> &Self::PermissionChecker {
        self.permissions.as_ref()
    }
}

impl DependOnPublicBaseUrl for TestFaultModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.public_base_url
    }
}

impl DependOnHttpSigner for TestFaultModule {
    type HttpSigner = HttpSignerImpl;

    fn http_signer(&self) -> &Self::HttpSigner {
        &self.http_signer
    }
}

async fn account_id_of(database: &PostgresDatabase, account_nanoid: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM accounts WHERE nanoid = $1")
        .bind(account_nanoid)
        .fetch_one(&mut *database.connection().await.unwrap())
        .await
        .unwrap()
}

/// Injects a remote actor into the test cache (no HTTP resolution) and
/// pre-inserts its `remote_accounts` row so follow rows can reference it
/// before the block use case runs its pre-transaction upsert.
async fn seed_remote_actor(database: &PostgresDatabase, inbox_url: &str) -> (String, i64) {
    let remote_id = kernel::generate_id();
    let actor_url = format!("http://remote-{remote_id}.example.invalid/users/actor");
    inject_test_remote_actor(
        &actor_url,
        &format!("actor-{remote_id}"),
        inbox_url,
        "unused-in-outbound-tests",
    );
    sqlx::query(
        "INSERT INTO remote_accounts (id, acct, url, inbox_url, public_key_pem) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(remote_id)
    .bind(format!(
        "actor-{remote_id}@remote-{remote_id}.example.invalid"
    ))
    .bind(&actor_url)
    .bind(inbox_url)
    .bind("unused-in-outbound-tests")
    .execute(&mut *database.connection().await.unwrap())
    .await
    .unwrap();
    (actor_url, remote_id)
}

async fn seed_follows_between(
    database: &PostgresDatabase,
    local_account_id: i64,
    remote_account_id: i64,
) {
    let mut conn = database.connection().await.unwrap();
    sqlx::query(
        "INSERT INTO follows (id, follower_local_id, followee_remote_id, approved_at) \
         VALUES ($1, $2, $3, NOW())",
    )
    .bind(kernel::generate_id())
    .bind(local_account_id)
    .bind(remote_account_id)
    .execute(&mut *conn)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO follows (id, follower_remote_id, followee_local_id, approved_at) \
         VALUES ($1, $2, $3, NOW())",
    )
    .bind(kernel::generate_id())
    .bind(remote_account_id)
    .bind(local_account_id)
    .execute(&mut *conn)
    .await
    .unwrap();
}

struct ReceivedHttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

async fn read_http_request(socket: &mut tokio::net::TcpStream) -> ReceivedHttpRequest {
    use tokio::io::AsyncReadExt;

    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        if let Some(pos) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break pos;
        }
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0, "connection closed before headers completed");
        buffer.extend_from_slice(&chunk[..read]);
    };
    let headers = String::from_utf8(buffer[..header_end].to_vec()).unwrap();
    let mut lines = headers.split("\r\n");
    let mut request_line = lines.next().unwrap().split_whitespace();
    let method = request_line.next().unwrap().to_string();
    let path = request_line.next().unwrap().to_string();
    let content_length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let body_start = header_end + 4;
    while buffer.len() - body_start < content_length {
        let read = socket.read(&mut chunk).await.unwrap();
        assert!(read > 0, "connection closed before body completed");
        buffer.extend_from_slice(&chunk[..read]);
    }
    ReceivedHttpRequest {
        method,
        path,
        body: buffer[body_start..body_start + content_length].to_vec(),
    }
}

async fn accept_once_respond_ok(listener: tokio::net::TcpListener) {
    use tokio::io::AsyncWriteExt;

    let (mut socket, _) = listener.accept().await.unwrap();
    let _request = read_http_request(&mut socket).await;
    socket
        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
        .await
        .unwrap();
}

async fn block_write_counts(
    database: &PostgresDatabase,
    blocker_id: i64,
    remote_id: i64,
) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT \
         (SELECT COUNT(*) FROM blocks WHERE blocker_local_id = $1 AND blocked_remote_id = $2), \
         (SELECT COUNT(*) FROM follows WHERE \
           (follower_local_id = $1 AND followee_remote_id = $2) \
           OR (follower_remote_id = $2 AND followee_local_id = $1)), \
         (SELECT COUNT(*) FROM outbox_activities WHERE account_id = $1)",
    )
    .bind(blocker_id)
    .bind(remote_id)
    .fetch_one(&mut *database.connection().await.unwrap())
    .await
    .unwrap()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn block_use_case_equivalence_preserves_state_effects() {
    // Given: a local blocker following (and followed by) a remote actor whose
    // inbox is unreachable, so post-commit delivery fails but is tolerated
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let blocker = create_test_account(&module, &auth_account_id).await;
    let (actor_url, remote_id) =
        seed_remote_actor(&module.database, "http://127.0.0.1:1/inbox").await;
    let blocker_id = account_id_of(&module.database, &blocker).await;
    seed_follows_between(&module.database, blocker_id, remote_id).await;

    // When
    let relation = module
        .block_account(
            auth_account_id,
            BlockAccountDto {
                account_nanoid: blocker,
                target: actor_url,
            },
        )
        .await
        .unwrap();

    // Then
    assert_eq!(relation.target_type, "remote");
    assert_eq!(
        block_write_counts(&module.database, blocker_id, remote_id).await,
        (1, 0, 1),
        "block row persisted, both-direction follows removed, one Block outbox row"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn block_use_case_rolls_back_all_writes_when_transaction_fails() {
    // Given: same setup as the equivalence test, but the transaction manager
    // injects a failure after the unit of work has run
    let (module, password_file, auth_account_id) = mute_test_module().await;
    let blocker = create_test_account(&module, &auth_account_id).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let inbox_url = format!(
        "http://127.0.0.1:{}/inbox",
        listener.local_addr().unwrap().port()
    );
    let (actor_url, remote_id) = seed_remote_actor(&module.database, &inbox_url).await;
    let blocker_id = account_id_of(&module.database, &blocker).await;
    seed_follows_between(&module.database, blocker_id, remote_id).await;
    // Delivery must never be attempted when the transaction fails; the
    // listener only exists so a regression toward pre-commit delivery fails
    // loudly instead of hanging.
    let server = tokio::spawn(accept_once_respond_ok(listener));
    let fault_module = TestFaultModule::new(password_file.path()).await;

    // When
    let result = fault_module
        .block_account(
            auth_account_id,
            BlockAccountDto {
                account_nanoid: blocker,
                target: actor_url,
            },
        )
        .await;

    // Then
    assert!(result.is_err());
    assert_eq!(
        block_write_counts(&module.database, blocker_id, remote_id).await,
        (0, 2, 0),
        "block row, follow removals and outbox row must roll back together"
    );
    server.abort();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn block_delivery_failure_leaves_retryable_outbox_record() {
    // Given: a remote actor whose inbox refuses connections
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let blocker = create_test_account(&module, &auth_account_id).await;
    let (actor_url, remote_id) =
        seed_remote_actor(&module.database, "http://127.0.0.1:1/inbox").await;
    let blocker_id = account_id_of(&module.database, &blocker).await;

    // When
    let relation = module
        .block_account(
            auth_account_id,
            BlockAccountDto {
                account_nanoid: blocker,
                target: actor_url,
            },
        )
        .await
        .unwrap();

    // Then: the operation succeeds, the block is committed, and the outbox
    // row records the failed attempt without leaving the delivered read path
    assert_eq!(relation.target_type, "remote");
    assert_eq!(
        block_write_counts(&module.database, blocker_id, remote_id)
            .await
            .0,
        1
    );
    let attempt_state: (bool, bool, bool) = sqlx::query_as(
        "SELECT delivered_at IS NULL, attempted_at IS NOT NULL, error IS NOT NULL \
         FROM outbox_activities WHERE account_id = $1 AND activity_type = 'Block'",
    )
    .bind(blocker_id)
    .fetch_one(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert_eq!(attempt_state, (true, true, true));
    let account_id = AccountId::new(blocker_id);
    let mut executor = module.database.connection().await.unwrap();
    assert!(module
        .outbox_activity_repository()
        .find_by_account_id(&mut executor, &account_id, 10, None)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        module
            .outbox_activity_repository()
            .count_by_account_id(&mut executor, &account_id)
            .await
            .unwrap(),
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn block_delivery_occurs_after_db_commit() {
    // Given: an in-test HTTP server acting as the remote inbox; it observes
    // the database from a fresh connection before accepting the activity
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let blocker = create_test_account(&module, &auth_account_id).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let inbox_url = format!(
        "http://127.0.0.1:{}/inbox",
        listener.local_addr().unwrap().port()
    );
    let (actor_url, _remote_id) = seed_remote_actor(&module.database, &inbox_url).await;

    let server = {
        let blocker = blocker.clone();
        let database = module.database.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;

            let (mut socket, _) = listener.accept().await.unwrap();
            let request = read_http_request(&mut socket).await;
            assert_eq!(
                (request.method.as_str(), request.path.as_str()),
                ("POST", "/inbox")
            );
            let activity: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(activity["type"], "Block");
            let block_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM blocks b JOIN accounts a ON a.id = b.blocker_local_id \
                 WHERE a.nanoid = $1",
            )
            .bind(&blocker)
            .fetch_one(&mut *database.connection().await.unwrap())
            .await
            .unwrap();
            assert_eq!(
                block_count, 1,
                "block row must be committed before the activity is delivered"
            );
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
                .await
                .unwrap();
        })
    };

    // When
    module
        .block_account(
            auth_account_id,
            BlockAccountDto {
                account_nanoid: blocker.clone(),
                target: actor_url,
            },
        )
        .await
        .unwrap();

    // Then
    server.await.unwrap();
    let delivered: (bool,) = sqlx::query_as(
        "SELECT delivered_at IS NOT NULL FROM outbox_activities o \
         JOIN accounts a ON a.id = o.account_id \
         WHERE a.nanoid = $1 AND o.activity_type = 'Block'",
    )
    .bind(&blocker)
    .fetch_one(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert!(delivered.0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn second_block_returns_rejected_already_blocked() {
    // Given
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let blocker = create_test_account(&module, &auth_account_id).await;
    let target = create_test_account(&module, &auth_account_id).await;
    let dto = || BlockAccountDto {
        account_nanoid: blocker.clone(),
        target: target.clone(),
    };
    module
        .block_account(auth_account_id.clone(), dto())
        .await
        .unwrap();

    // When
    let second = module.block_account(auth_account_id, dto()).await;

    // Then
    let Err(error) = second else {
        panic!("second block must be rejected");
    };
    assert!(matches!(error.current_context(), KernelError::Rejected));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unblock_missing_returns_not_found() {
    // Given
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let blocker = create_test_account(&module, &auth_account_id).await;
    let target = create_test_account(&module, &auth_account_id).await;

    // When
    let result = module
        .unblock_account(
            auth_account_id,
            BlockAccountDto {
                account_nanoid: blocker,
                target,
            },
        )
        .await;

    // Then
    let Err(error) = result else {
        panic!("unblock without a block must be not found");
    };
    assert!(matches!(error.current_context(), KernelError::NotFound));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unblock_equivalence_removes_block_and_creates_undo_outbox() {
    // Given: a blocked remote actor (unreachable inbox tolerated for both
    // the Block and the Undo delivery)
    let (module, _password_file, auth_account_id) = mute_test_module().await;
    let blocker = create_test_account(&module, &auth_account_id).await;
    let (actor_url, remote_id) =
        seed_remote_actor(&module.database, "http://127.0.0.1:1/inbox").await;
    let blocker_id = account_id_of(&module.database, &blocker).await;
    let dto = || BlockAccountDto {
        account_nanoid: blocker.clone(),
        target: actor_url.clone(),
    };
    module
        .block_account(auth_account_id.clone(), dto())
        .await
        .unwrap();

    // When
    module
        .unblock_account(auth_account_id, dto())
        .await
        .unwrap();

    // Then
    assert_eq!(
        block_write_counts(&module.database, blocker_id, remote_id)
            .await
            .0,
        0,
        "block row removed"
    );
    let activity_types: Vec<(String,)> = sqlx::query_as(
        "SELECT activity_type FROM outbox_activities WHERE account_id = $1 ORDER BY id",
    )
    .bind(blocker_id)
    .fetch_all(&mut *module.database.connection().await.unwrap())
    .await
    .unwrap();
    assert_eq!(
        activity_types,
        vec![("Block".to_string(),), ("Undo".to_string(),)]
    );
}
