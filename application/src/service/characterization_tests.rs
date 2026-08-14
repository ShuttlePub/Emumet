use super::account::CreateAccountUseCase;
use super::account_detail::UpdateAccountDetailUseCase;
use crate::transfer::account::{AccountFieldDto, CreateAccountDto, UpdateAccountDto};
use adapter::processor::metadata::DependOnMetadataSignal;
use adapter::processor::profile::DependOnProfileSignal;
use driver::crypto::{Argon2Encryptor, FilePasswordProvider, Rsa2048RawGenerator};
use driver::database::PostgresDatabase;
use kernel::interfaces::config::{DependOnPublicBaseUrl, PublicBaseUrl};
use kernel::interfaces::crypto::{
    DependOnKeyEncryptor, DependOnPasswordProvider, DependOnRawKeyGenerator,
};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::permission::{
    DependOnPermissionChecker, DependOnPermissionWriter, InstanceRole, PermissionChecker,
    PermissionReq, PermissionWriter, RelationTarget,
};
use kernel::interfaces::signal::Signal;
use kernel::prelude::entity::{AuthAccountId, FieldAction};
use kernel::KernelError;
use std::sync::atomic::{AtomicUsize, Ordering};

struct NoopSignal;

impl<ID: Send> Signal<ID> for NoopSignal {
    async fn emit(&self, _signal_id: ID) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

#[derive(Default)]
struct AllowPermissions {
    writes: AtomicUsize,
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
        _target: &RelationTarget,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn delete_relation(
        &self,
        _target: &RelationTarget,
        _subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}

struct TestModule {
    database: PostgresDatabase,
    password_provider: FilePasswordProvider,
    raw_key_generator: Rsa2048RawGenerator,
    key_encryptor: Argon2Encryptor,
    permissions: AllowPermissions,
    public_base_url: PublicBaseUrl,
    signal: NoopSignal,
}

impl TestModule {
    async fn new(password_path: &std::path::Path) -> Self {
        Self {
            database: PostgresDatabase::new().await.unwrap(),
            password_provider: FilePasswordProvider::with_paths(password_path, password_path),
            raw_key_generator: Rsa2048RawGenerator,
            key_encryptor: Argon2Encryptor::default(),
            permissions: AllowPermissions::default(),
            public_base_url: PublicBaseUrl::new("https://example.com".to_string()),
            signal: NoopSignal,
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
        sqlx::query(
            "INSERT INTO auth_accounts (id, host_id, client_id, version) VALUES ($1, $2, $3, 0)",
        )
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
        &self.password_provider
    }
}

impl DependOnRawKeyGenerator for TestModule {
    type RawKeyGenerator = Rsa2048RawGenerator;

    fn raw_key_generator(&self) -> &Self::RawKeyGenerator {
        &self.raw_key_generator
    }
}

impl DependOnKeyEncryptor for TestModule {
    type KeyEncryptor = Argon2Encryptor;

    fn key_encryptor(&self) -> &Self::KeyEncryptor {
        &self.key_encryptor
    }
}

impl DependOnPermissionChecker for TestModule {
    type PermissionChecker = AllowPermissions;

    fn permission_checker(&self) -> &Self::PermissionChecker {
        &self.permissions
    }
}

impl DependOnPermissionWriter for TestModule {
    type PermissionWriter = AllowPermissions;

    fn permission_writer(&self) -> &Self::PermissionWriter {
        &self.permissions
    }
}

impl DependOnPublicBaseUrl for TestModule {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.public_base_url
    }
}

impl DependOnProfileSignal for TestModule {
    type ProfileSignal = NoopSignal;

    fn profile_signal(&self) -> &Self::ProfileSignal {
        &self.signal
    }
}

impl DependOnMetadataSignal for TestModule {
    type MetadataSignal = NoopSignal;

    fn metadata_signal(&self) -> &Self::MetadataSignal {
        &self.signal
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
