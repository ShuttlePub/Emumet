use super::*;
use crate::database::postgres::isolated_db::IsolatedDatabase;
use kernel::interfaces::database::{
    DatabaseConnection, Transaction, TransactionManager, TransactionalDatabaseConnection,
};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::interfaces::repository::{DependOnAccountRepository, DependOnProfileRepository};
use kernel::prelude::entity::{AccountIsBot, AccountName, Nanoid, Profile};
use std::time::Duration;

async fn fixture(db: &PostgresDatabase, count: usize) -> (Account, Vec<Profile>) {
    kernel::ensure_generator_initialized();
    let mut c = db.connection().await.unwrap();
    let id = AccountId::default();
    let command = if count > 1 {
        Account::create_organization(
            id.clone(),
            AccountName::new(format!("l1{}", id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::default(),
            AuthAccountId::default(),
        )
    } else {
        Account::create(
            id.clone(),
            AccountName::new(format!("l1{}", id.as_ref())),
            AccountIsBot::new(false),
            Nanoid::default(),
            AuthAccountId::default(),
        )
    };
    let event = db.account_repository().save(&mut c, command).await.unwrap();
    let mut a = None;
    Account::apply(&mut a, event).unwrap();
    let a = a.unwrap();
    db.account_read_model().create(&mut c, &a).await.unwrap();
    let mut profiles = vec![];
    for _ in 0..count {
        let event = db
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
        Profile::apply(&mut p, event).unwrap();
        let p = p.unwrap();
        db.profile_read_model().create(&mut c, &p).await.unwrap();
        profiles.push(p);
    }
    (a, profiles)
}
async fn gated(
    db: &PostgresDatabase,
    p: &Profile,
) -> crate::database::postgres::PostgresTransaction {
    let mut tx = db.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_accounts(tx.connection(), std::slice::from_ref(p.account_id()))
        .await
        .unwrap();
    PostgresHomeLinkStore
        .lock_profiles(tx.connection(), std::slice::from_ref(p.id()))
        .await
        .unwrap();
    tx
}
async fn register(
    db: &PostgresDatabase,
    p: &Profile,
    origin: &str,
) -> error_stack::Result<(HomeLink, bool), KernelError> {
    let mut tx = gated(db, p).await;
    let o = PostgresHomeLinkStore
        .owner(tx.connection(), p.id(), p.account_id())
        .await?;
    let result = PostgresHomeLinkStore
        .register(
            tx.connection(),
            &o,
            "https://emumet.example/ap/profiles/test",
            origin,
        )
        .await?;
    tx.commit().await?;
    Ok(result)
}
async fn links(db: &PostgresDatabase, p: &Profile) -> Vec<HomeLink> {
    let mut tx = gated(db, p).await;
    let result = PostgresHomeLinkStore
        .list(tx.connection(), p.id())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    result
}
async fn deactivate(db: &PostgresDatabase, a: &Account) -> EventVersion<Account> {
    let mut c = db.connection().await.unwrap();
    let loaded = db.account_repository().load(&mut c, a.id()).await.unwrap();
    db.account_repository()
        .save(
            &mut c,
            Account::deactivate(a.id().clone(), loaded.version().clone()),
        )
        .await
        .unwrap()
        .version
}
async fn reactivate(db: &PostgresDatabase, a: &Account) {
    let mut c = db.connection().await.unwrap();
    let loaded = db.account_repository().load(&mut c, a.id()).await.unwrap();
    db.account_repository()
        .save(
            &mut c,
            Account::reactivate(a.id().clone(), loaded.version().clone()),
        )
        .await
        .unwrap();
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn concurrent_registration_has_one_active_home() {
    let w = IsolatedDatabase::create().await;
    let (_, p) = fixture(&w.database, 1).await;
    let p = p[0].clone();
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
    let mut tasks = vec![];
    for _ in 0..12 {
        let db = w.database.clone();
        let p = p.clone();
        let b = barrier.clone();
        tasks.push(tokio::spawn(async move {
            b.wait().await;
            register(&db, &p, "https://home.example").await.unwrap()
        }));
    }
    let mut created = 0;
    let mut ids = std::collections::BTreeSet::new();
    for task in tasks {
        let (l, new) = task.await.unwrap();
        created += usize::from(new);
        ids.insert(l.id);
    }
    assert_eq!(created, 1);
    assert_eq!(ids.len(), 1);
    assert_eq!(links(&w.database, &p).await.len(), 1);
    assert_eq!(
        register(&w.database, &p, "https://other.example")
            .await
            .unwrap_err()
            .current_context(),
        &KernelError::Concurrency
    );
    let bad=sqlx::query("INSERT INTO profile_links SELECT id||'x',profile_id,owner_account_id,registered_account_version,actor_snapshot,origin,trust_mode,generation,state,created_at,unlinked_at,unlink_reason,deactivated_account_id,deactivated_account_version FROM profile_links").execute(&w.pool).await;
    assert!(bad.is_err());
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unlink_is_idempotent_old_id_does_not_unlink_new_home() {
    let w = IsolatedDatabase::create().await;
    let (_, ps) = fixture(&w.database, 1).await;
    let p = &ps[0];
    let old = register(&w.database, p, "https://home.example")
        .await
        .unwrap()
        .0;
    for _ in 0..2 {
        let mut tx = gated(&w.database, p).await;
        PostgresHomeLinkStore
            .unlink(tx.connection(), p.id(), &old.id)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    assert_eq!(links(&w.database, p).await[0].generation, 2);
    let new = register(&w.database, p, "https://other.example")
        .await
        .unwrap()
        .0;
    let mut tx = gated(&w.database, p).await;
    PostgresHomeLinkStore
        .unlink(tx.connection(), p.id(), &old.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_ne!(old.id, new.id);
    assert!(links(&w.database, p)
        .await
        .iter()
        .any(|l| l.id == new.id && l.generation == 1 && l.state == "active"));
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn deactivation_commit_revokes_all_profiles_with_rows_remaining_and_fences_replay() {
    let w = IsolatedDatabase::create().await;
    let (a, ps) = fixture(&w.database, 3).await;
    for p in &ps {
        register(&w.database, p, "https://home.example")
            .await
            .unwrap();
    }
    let v = deactivate(&w.database, &a).await;
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM profiles WHERE account_id=$1")
        .bind(a.id().as_ref())
        .fetch_one(&w.pool)
        .await
        .unwrap();
    assert_eq!(n, 3);
    for p in &ps {
        let l = links(&w.database, p).await;
        assert_eq!(l[0].state, "unlinked");
        assert_eq!(l[0].generation, 2);
        assert!(register(&w.database, p, "https://home.example")
            .await
            .is_err());
    }
    reactivate(&w.database, &a).await;
    for p in &ps {
        assert_eq!(links(&w.database, p).await[0].state, "unlinked");
        register(&w.database, p, "https://new.example")
            .await
            .unwrap();
    }
    let mut tx = w.database.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_accounts(tx.connection(), std::slice::from_ref(a.id()))
        .await
        .unwrap();
    for _ in 0..2 {
        PostgresHomeLinkStore
            .deactivate(tx.connection(), a.id(), &v)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    for p in &ps {
        let ls = links(&w.database, p).await;
        assert_eq!(ls.iter().filter(|l| l.state == "active").count(), 1);
        assert!(ls
            .iter()
            .any(|l| l.origin == "https://new.example" && l.generation == 1));
    }
    deactivate(&w.database, &a).await;
    let mut tx = w.database.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_accounts(tx.connection(), std::slice::from_ref(a.id()))
        .await
        .unwrap();
    PostgresHomeLinkStore
        .delete_owned_profiles(tx.connection(), a.id())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profile_links")
            .fetch_one(&w.pool)
            .await
            .unwrap(),
        6
    );
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn write_side_ban_is_seen_while_projection_is_active() {
    let w = IsolatedDatabase::create().await;
    let (a, ps) = fixture(&w.database, 1).await;
    let p = &ps[0];
    let l = register(&w.database, p, "https://home.example")
        .await
        .unwrap()
        .0;
    let mut c = w.database.connection().await.unwrap();
    let loaded = w
        .database
        .account_repository()
        .load(&mut c, a.id())
        .await
        .unwrap();
    w.database
        .account_repository()
        .save(
            &mut c,
            Account::ban(a.id().clone(), "test".to_owned(), loaded.version().clone()),
        )
        .await
        .unwrap();
    let projection = w
        .database
        .account_read_model()
        .find_by_id_including_deleted(&mut c, a.id())
        .await
        .unwrap()
        .unwrap();
    assert!(projection.status().is_active());
    let mut tx = gated(&w.database, p).await;
    assert!(
        !PostgresHomeLinkStore
            .owner(tx.connection(), p.id(), p.account_id())
            .await
            .unwrap()
            .active
    );
    tx.commit().await.unwrap();
    assert_eq!(links(&w.database, p).await[0], l);
}
async fn mock_authorize(db: &PostgresDatabase, p: &Profile, id: &str) -> bool {
    let mut tx = gated(db, p).await;
    let owner = PostgresHomeLinkStore
        .owner(tx.connection(), p.id(), p.account_id())
        .await
        .unwrap();
    let allowed = owner.active
        && PostgresHomeLinkStore
            .list(tx.connection(), p.id())
            .await
            .unwrap()
            .iter()
            .any(|l| l.id == id && l.state == "active");
    if allowed {
        sqlx::query("INSERT INTO l1_test_permits(link_id) VALUES ($1)")
            .bind(id)
            .execute(&mut **tx.connection())
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    allowed
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn p0_and_unlink_precommit_stop_points_serialize_mock_authorization() {
    let w = IsolatedDatabase::create().await;
    let (_, ps) = fixture(&w.database, 1).await;
    let p = ps[0].clone();
    let l = register(&w.database, &p, "https://home.example")
        .await
        .unwrap()
        .0;
    sqlx::query("CREATE TABLE l1_test_permits(link_id text)")
        .execute(&w.pool)
        .await
        .unwrap();
    // P0: claimed without a permit. Revoker stopped after UPDATE, before commit.
    let mut unlink = gated(&w.database, &p).await;
    PostgresHomeLinkStore
        .unlink(unlink.connection(), p.id(), &l.id)
        .await
        .unwrap();
    let db = w.database.clone();
    let p2 = p.clone();
    let id = l.id.clone();
    let task = tokio::spawn(async move { mock_authorize(&db, &p2, &id).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!task.is_finished());
    unlink.commit().await.unwrap();
    assert!(!task.await.unwrap());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM l1_test_permits")
            .fetch_one(&w.pool)
            .await
            .unwrap(),
        0
    );
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn p1_stalled_permit_rolls_back_and_bounds_revocation_wait() {
    let w = IsolatedDatabase::create().await;
    let (a, ps) = fixture(&w.database, 1).await;
    let p = ps[0].clone();
    let l = register(&w.database, &p, "https://home.example")
        .await
        .unwrap()
        .0;
    sqlx::query("CREATE TABLE l1_test_permits(link_id text)")
        .execute(&w.pool)
        .await
        .unwrap();
    let (send, recv) = tokio::sync::oneshot::channel();
    let db = w.database.clone();
    let a2 = a.clone();
    let stalled = tokio::spawn(async move {
        db.transaction(move |c| {
            Box::pin(async move {
                PostgresHomeLinkStore
                    .lock_accounts(c, std::slice::from_ref(a2.id()))
                    .await?;
                PostgresHomeLinkStore
                    .lock_profiles(c, std::slice::from_ref(p.id()))
                    .await?;
                sqlx::query("INSERT INTO l1_test_permits(link_id) VALUES ($1)")
                    .bind(&l.id)
                    .execute(&mut **c)
                    .await
                    .convert_error()?;
                send.send(()).unwrap();
                std::future::pending::<()>().await;
                Ok(())
            })
        })
        .await
    });
    recv.await.unwrap();
    let now = std::time::Instant::now();
    let mut tx = w.database.get_transaction().await.unwrap();
    assert!(PostgresHomeLinkStore
        .lock_accounts(tx.connection(), std::slice::from_ref(a.id()))
        .await
        .is_err());
    drop(tx);
    assert!(now.elapsed() < Duration::from_secs(4));
    assert!(stalled.await.unwrap().is_err());
    deactivate(&w.database, &a).await;
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM l1_test_permits")
            .fetch_one(&w.pool)
            .await
            .unwrap(),
        0
    );
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn lifecycle_timeouts_are_transaction_local_and_plain_transactions_remain_unbounded() {
    let w = IsolatedDatabase::create().await;
    let (a, _) = fixture(&w.database, 1).await;
    let mut c = w.database.connection().await.unwrap();
    let baseline: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    let mut tx = w.database.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_accounts(tx.connection(), std::slice::from_ref(a.id()))
        .await
        .unwrap();
    for (name, value) in [
        ("lock_timeout", "2s"),
        ("statement_timeout", "3s"),
        ("idle_in_transaction_session_timeout", "5s"),
    ] {
        let actual: String = sqlx::query_scalar(&format!("SHOW {name}"))
            .fetch_one(&mut **tx.connection())
            .await
            .unwrap();
        assert_eq!(actual, value);
    }
    tx.commit().await.unwrap();
    let after: String = sqlx::query_scalar("SHOW statement_timeout")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    assert_eq!(after, baseline);
    w.database
        .transaction(|_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(5100)).await;
                Ok(())
            })
        })
        .await
        .unwrap();
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn additive_migration_on_populated_main_schema_and_old_binary_refusal() {
    let w = IsolatedDatabase::create_before_home_links().await;
    let (a, ps) = fixture(&w.database, 1).await;
    let p = &ps[0];
    let before: (i64, String, i64) =
        sqlx::query_as("SELECT id,nanoid,version FROM profiles WHERE id=$1")
            .bind(p.id().as_ref())
            .fetch_one(&w.pool)
            .await
            .unwrap();
    w.apply_reset().await;
    assert_eq!(
        sqlx::query_as::<_, (i64, String, i64)>(
            "SELECT id,nanoid,version FROM profiles WHERE id=$1"
        )
        .bind(p.id().as_ref())
        .fetch_one(&w.pool)
        .await
        .unwrap(),
        before
    );
    register(&w.database, p, "https://home.example")
        .await
        .unwrap();
    deactivate(&w.database, &a).await;
    // Older sqlx migrators reject the extra migration, without undoing the durable ledger.
    let full = sqlx::migrate!("../migrations");
    let old = sqlx::migrate::Migrator {
        migrations: std::borrow::Cow::Owned(
            full.migrations
                .iter()
                .filter(|m| m.version != 20261122000200)
                .cloned()
                .collect(),
        ),
        ignore_missing: false,
        locking: true,
    };
    assert!(matches!(
        old.run(&w.pool).await,
        Err(sqlx::migrate::MigrateError::VersionMissing(20261122000200))
    ));
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profile_links")
            .fetch_one(&w.pool)
            .await
            .unwrap(),
        1
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn suspension_uses_committed_state_and_expiry_without_unlinking() {
    let w = IsolatedDatabase::create().await;
    let (a, ps) = fixture(&w.database, 1).await;
    let p = &ps[0];
    let original = register(&w.database, p, "https://home.example")
        .await
        .unwrap()
        .0;
    for (expiry, expected) in [
        (None, false),
        (
            Some(time::OffsetDateTime::now_utc() + time::Duration::hours(1)),
            false,
        ),
        (
            Some(time::OffsetDateTime::now_utc() - time::Duration::seconds(1)),
            true,
        ),
    ] {
        let mut c = w.database.connection().await.unwrap();
        let loaded = w
            .database
            .account_repository()
            .load(&mut c, a.id())
            .await
            .unwrap();
        w.database
            .account_repository()
            .save(
                &mut c,
                Account::suspend(
                    a.id().clone(),
                    "test".into(),
                    expiry,
                    loaded.version().clone(),
                ),
            )
            .await
            .unwrap();
        let mut tx = gated(&w.database, p).await;
        assert_eq!(
            PostgresHomeLinkStore
                .owner(tx.connection(), p.id(), a.id())
                .await
                .unwrap()
                .active,
            expected
        );
        assert_eq!(
            PostgresHomeLinkStore
                .list(tx.connection(), p.id())
                .await
                .unwrap(),
            vec![original.clone()]
        );
        tx.commit().await.unwrap();
        let loaded = w
            .database
            .account_repository()
            .load(&mut c, a.id())
            .await
            .unwrap();
        w.database
            .account_repository()
            .save(
                &mut c,
                Account::unsuspend(a.id().clone(), loaded.version().clone()),
            )
            .await
            .unwrap();
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn gate_budget_also_counts_time_spent_before_the_gate() {
    let w = IsolatedDatabase::create().await;
    let (a, _) = fixture(&w.database, 1).await;
    let began = tokio::time::Instant::now();
    let result = w
        .database
        .transaction(move |c| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_secs(4)).await;
                PostgresHomeLinkStore
                    .lock_accounts(c, std::slice::from_ref(a.id()))
                    .await?;
                tokio::time::sleep(Duration::from_secs(2)).await;
                Ok(())
            })
        })
        .await;
    assert!(result.is_err());
    assert!(
        began.elapsed() < Duration::from_millis(5800),
        "the 5s budget must not restart when a gate is acquired"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn deactivation_waits_for_existing_relationship_gate_before_locking_links() {
    let w = IsolatedDatabase::create().await;
    let (a, ps) = fixture(&w.database, 1).await;
    let p = &ps[0];
    let link = register(&w.database, p, "https://home.example")
        .await
        .unwrap()
        .0;
    let mut relationship = w.database.get_transaction().await.unwrap();
    PostgresProfileRelationshipLock
        .lock_profiles(relationship.connection(), std::slice::from_ref(p.id()))
        .await
        .unwrap();
    let db = w.database.clone();
    let task = tokio::spawn(async move { deactivate(&db, &a).await });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!task.is_finished());
    let mut probe = w.database.get_transaction().await.unwrap();
    sqlx::query("SELECT id FROM profile_links WHERE id=$1 FOR UPDATE NOWAIT")
        .bind(&link.id)
        .fetch_one(&mut **probe.connection())
        .await
        .expect("link must not be locked while waiting on the earlier relationship gate");
    probe.commit().await.unwrap();
    relationship.commit().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(links(&w.database, p).await[0].state, "unlinked");
}
