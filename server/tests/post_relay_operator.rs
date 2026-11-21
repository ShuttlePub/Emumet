//! Shipped server commands; optionally the same cases run against a locally built release image.
use application::instance_identity::{
    DependOnInstanceKeyCache, EnsureInstanceIdentityUseCase, InstanceKeyCache,
};
use driver::{
    crypto::*,
    database::{isolated_db::IsolatedDatabase, PostgresDatabase},
};
use kernel::{
    interfaces::{
        config::{DependOnPublicBaseUrl, PublicBaseUrl},
        crypto::*,
        database::*,
        event::EventApplier,
        home_link::{DependOnHomeLinkStore, HomeLinkStore},
        post_relay::*,
        read_model::*,
        repository::*,
    },
    prelude::entity::*,
};
use std::{process::Output, sync::Arc};
#[derive(Clone)]
struct Keys {
    db: PostgresDatabase,
    password: FilePasswordProvider,
    cache: Arc<InstanceKeyCache>,
    encryptor: Argon2Encryptor,
    origin: PublicBaseUrl,
}
kernel::impl_database_delegation!(Keys, db, PostgresDatabase);
impl DependOnPasswordProvider for Keys {
    type PasswordProvider = FilePasswordProvider;
    fn password_provider(&self) -> &FilePasswordProvider {
        &self.password
    }
}
impl DependOnKeyEncryptor for Keys {
    type KeyEncryptor = Argon2Encryptor;
    fn key_encryptor(&self) -> &Argon2Encryptor {
        &self.encryptor
    }
}
impl DependOnRawKeyGenerator for Keys {
    type RawKeyGenerator = Rsa2048RawGenerator;
    fn raw_key_generator(&self) -> &Rsa2048RawGenerator {
        &Rsa2048RawGenerator
    }
}
impl DependOnPreparedSignerFactory for Keys {
    type PreparedSignerFactory = Rsa2048PreparedSignerFactory;
    fn prepared_signer_factory(&self) -> &Rsa2048PreparedSignerFactory {
        &Rsa2048PreparedSignerFactory
    }
}
impl DependOnSignatureVerifier for Keys {
    type SignatureVerifier = Rsa2048Verifier;
    fn signature_verifier(&self) -> &Rsa2048Verifier {
        &Rsa2048Verifier
    }
}
impl DependOnPublicBaseUrl for Keys {
    fn public_base_url(&self) -> &PublicBaseUrl {
        &self.origin
    }
}
impl DependOnInstanceKeyCache for Keys {
    fn instance_key_cache(&self) -> &InstanceKeyCache {
        &self.cache
    }
}
async fn fixture(db: &PostgresDatabase, count: usize, linked: bool) -> (Account, Vec<Profile>) {
    kernel::ensure_generator_initialized();
    let mut c = db.connection().await.unwrap();
    let id = AccountId::default();
    let command = if count > 1 {
        Account::create_organization(
            id.clone(),
            AccountName::new(format!("relay{}", id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::default(),
            AuthAccountId::default(),
        )
    } else {
        Account::create(
            id.clone(),
            AccountName::new(format!("relay{}", id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::default(),
            AuthAccountId::default(),
        )
    };
    let e = db.account_repository().save(&mut c, command).await.unwrap();
    let mut a = None;
    Account::apply(&mut a, e).unwrap();
    let a = a.unwrap();
    db.account_read_model().create(&mut c, &a).await.unwrap();
    let mut profiles = vec![];
    for _ in 0..count {
        let e = db
            .profile_repository()
            .save(
                &mut c,
                Profile::create(
                    ProfileId::new(kernel::generate_id()),
                    id.clone(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                ),
            )
            .await
            .unwrap();
        let mut p = None;
        Profile::apply(&mut p, e).unwrap();
        let p = p.unwrap();
        let mut t = db.get_transaction().await.unwrap();
        db.home_link_store()
            .lock_accounts(t.connection(), std::slice::from_ref(&id))
            .await
            .unwrap();
        db.profile_read_model()
            .create(t.connection(), &p)
            .await
            .unwrap();
        sqlx::query("UPDATE profiles SET username=$2 WHERE id=$1")
            .bind(p.id().as_ref())
            .bind(format!("relay_{}", p.id().as_ref()))
            .execute(&mut **t.connection())
            .await
            .unwrap();
        db.home_link_store()
            .lock_profiles(t.connection(), std::slice::from_ref(p.id()))
            .await
            .unwrap();
        if linked {
            let o = db
                .home_link_store()
                .owner(t.connection(), p.id(), &id)
                .await
                .unwrap();
            db.home_link_store()
                .register(
                    t.connection(),
                    &o,
                    "https://emumet.example/ap/profiles/relay",
                    "https://home.example",
                )
                .await
                .unwrap();
        }
        t.commit().await.unwrap();
        profiles.push(p);
    }
    (a, profiles)
}

fn url(db: &IsolatedDatabase) -> String {
    let mut u = url::Url::parse(&std::env::var("DATABASE_URL").unwrap()).unwrap();
    u.set_path(db.pool.connect_options().get_database().unwrap());
    u.to_string()
}
async fn run(
    args: &[&str],
    database: &str,
    password: Option<&std::path::Path>,
    enabled: bool,
) -> Output {
    let mut cmd = if let Ok(image) = std::env::var("S1_OPERATOR_IMAGE") {
        // Explicit opt-in only. Run this test via a reviewed unsandboxed shell, never CI by default.
        let mut c = tokio::process::Command::new("docker");
        c.args(["run", "--rm", "--network", "none"]);
        c.args([
            "--user",
            &std::env::var("S1_OPERATOR_UID").expect("explicit local image UID required"),
        ]);
        let socket = std::env::var("S1_OPERATOR_SOCKET").expect("socket mount required");
        c.args(["--mount", &format!("type=bind,src={socket},dst={socket}")]);
        c.args([
            "-e",
            &format!("DATABASE_URL={database}"),
            "-e",
            "PUBLIC_BASE_URL=https://emumet.example",
            "-e",
            "POST_RELAY_ALLOWED_ORIGINS=[\"https://home.example\"]",
            "-e",
            &format!("POST_RELAY_SENDER_ENABLED={enabled}"),
        ]);
        if let Some(p) = password {
            c.args([
                "--mount",
                &format!(
                    "type=bind,readonly,src={},dst=/run/secrets/master-key-password",
                    p.display()
                ),
            ]);
        }
        c.arg(image);
        c
    } else {
        let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_server"));
        c.env("DATABASE_URL", database)
            .env("PUBLIC_BASE_URL", "https://emumet.example")
            .env("POST_RELAY_ALLOWED_ORIGINS", "[\"https://home.example\"]")
            .env("POST_RELAY_SENDER_ENABLED", enabled.to_string());
        // No source/Cargo is used by the binary; an isolated cwd holds just the password fixture.
        if let Some(p) = password {
            c.current_dir(p.parent().unwrap());
        }
        c
    };
    cmd.args(args)
        .env("WORKER_ID", "not-a-worker-id")
        .env("HYDRA_ISSUER_URL", "http://127.0.0.1:9")
        .env("AWS_EC2_METADATA_DISABLED", "false")
        .env("HTTP_LISTEN_ADDR", "not-an-address")
        .kill_on_drop(true);
    tokio::time::timeout(std::time::Duration::from_secs(15), cmd.output())
        .await
        .expect("operator started unrelated services or waited for HTTP")
        .unwrap()
}
#[tokio::test]
async fn post_relay_operator_help_is_available_without_ory_source_or_database() {
    let o = run(
        &["post-relay", "--help"],
        "postgres://bad@127.0.0.1:9/none",
        None,
        true,
    )
    .await;
    assert!(o.status.success());
    let h = String::from_utf8(o.stdout).unwrap();
    assert!(h.contains("no Cargo/source"));
    assert!(h.contains("UUID"));
}
#[tokio::test]
#[test_with::env(DATABASE_URL)]
async fn post_relay_operator_preview_confirm_discard_empty_redelivery_and_exact_schema_no_worker_id(
) {
    let d = IsolatedDatabase::create().await;
    let db = &d.database;
    let (_, ps) = fixture(db, 1, true).await;
    let p = &ps[0];
    let mut t = db.get_transaction().await.unwrap();
    let now = db
        .post_relay_store()
        .admission_time(t.connection())
        .await
        .unwrap();
    db.post_relay_store()
        .report_capacity(
            t.connection(),
            &CapacityReport {
                free_bytes: 50 * 1024 * 1024 * 1024,
                total_bytes: 100 * 1024 * 1024 * 1024,
                sampled_at: now,
            },
        )
        .await
        .unwrap();
    let recipient = db
        .post_relay_store()
        .prepare_admission(t.connection(), p.id())
        .await
        .unwrap();
    assert_eq!(
        db.post_relay_store()
            .enqueue(
                t.connection(),
                &RelayAdmission {
                    profile_id: p.id().clone(),
                    receipt: "discard-me".into(),
                    received_at: now,
                    recipient,
                    envelope: b"opaque-body".to_vec()
                }
            )
            .await
            .unwrap(),
        Admission::Queued
    );
    t.commit().await.unwrap();
    let mut t = db.get_transaction().await.unwrap();
    let l = db
        .post_relay_store()
        .claim_due(t.connection(), 1)
        .await
        .unwrap()
        .remove(0);
    t.commit().await.unwrap();
    let mut t = db.get_transaction().await.unwrap();
    db.post_relay_store()
        .record_attempt(
            t.connection(),
            &l,
            AttemptResult::Permanent { status: Some(403) },
            None,
            0,
        )
        .await
        .unwrap();
    t.commit().await.unwrap();
    let profile = p.id().as_ref().to_string();
    let op = "14a75dc9-81ad-4e41-b9ed-3e826f62f752";
    let database = url(&d);
    let o = run(
        &[
            "post-relay",
            "preview",
            "--action",
            "discard",
            "--profile-id",
            &profile,
            "--receipt",
            "discard-me",
            "--operation",
            op,
            "--operator",
            "ops",
            "--reason",
            "incident",
        ],
        &database,
        None,
        false,
    )
    .await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let preview: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(preview["count"], 1);
    let confirm = preview["confirmation"].as_str().unwrap();
    let o = run(
        &[
            "post-relay",
            "discard",
            "--profile-id",
            &profile,
            "--receipt",
            "discard-me",
            "--operation",
            op,
            "--operator",
            "ops",
            "--reason",
            "incident",
            "--confirm",
            "0:0:0",
        ],
        &database,
        None,
        false,
    )
    .await;
    assert!(!o.status.success());
    let o = run(
        &[
            "post-relay",
            "discard",
            "--profile-id",
            &profile,
            "--receipt",
            "discard-me",
            "--operation",
            op,
            "--operator",
            "ops",
            "--reason",
            "incident",
            "--confirm",
            confirm,
        ],
        &database,
        None,
        false,
    )
    .await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let bodies: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
        .fetch_one(&d.pool)
        .await
        .unwrap();
    assert_eq!(bodies, 0);
    let audit: (i64, String) =
        sqlx::query_as("SELECT discarded,reason FROM post_relay_operations WHERE id=$1")
            .bind(op)
            .fetch_one(&d.pool)
            .await
            .unwrap();
    assert_eq!(audit, (1, "incident".into()));
    let tmp = tempfile::tempdir().unwrap();
    let password = tmp.path().join("master-key-password");
    std::fs::write(&password, b"test-password").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    let keys = Keys {
        db: db.clone(),
        password: FilePasswordProvider::with_paths(&password, &password),
        cache: Arc::new(InstanceKeyCache::default()),
        encryptor: Argon2Encryptor::default(),
        origin: PublicBaseUrl::new("https://emumet.example".into()),
    };
    keys.ensure_instance_identity().await.unwrap();
    let o = run(
        &[
            "post-relay",
            "redeliver",
            "--profile-id",
            &profile,
            "--operation",
            "24a75dc9-81ad-4e41-b9ed-3e826f62f752",
            "--operator",
            "ops",
            "--reason",
            "recovered",
            "--confirm",
            "0:0:0",
        ],
        &database,
        Some(&password),
        true,
    )
    .await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // Incompatible schema must not be migrated, even with command credentials and valid keys.
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version=20261122000400")
        .execute(&d.pool)
        .await
        .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&d.pool)
        .await
        .unwrap();
    let o = run(
        &["post-relay", "status", "--operation", op],
        &database,
        None,
        true,
    )
    .await;
    assert!(!o.status.success());
    assert!(!String::from_utf8_lossy(&o.stderr).contains("opaque-body"));
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&d.pool)
        .await
        .unwrap();
    assert_eq!(before, after);
}
