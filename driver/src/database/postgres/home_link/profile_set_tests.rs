use super::*;
use kernel::interfaces::database::Savepoint;
use kernel::interfaces::projection::{DependOnProfileProjectionWriter, ProfileProjectionWriter};
use kernel::test_utils::ProfileBuilder;

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn content_update_never_inserts_or_changes_owner_or_rewinds_version() {
    let isolated = IsolatedDatabase::create().await;
    let db = &isolated.database;
    let (a, profiles) = fixture(db, 2).await;
    let (b, _) = fixture(db, 1).await;
    let p = &profiles[0];
    let v = *p.version().as_ref();
    let snapshot = |id, owner, version| {
        ProfileBuilder::new()
            .id(id)
            .account_id(owner)
            .version(kernel::prelude::entity::EventVersion::new(version))
            .build()
    };
    let mut c = db.connection().await.unwrap();
    let missing = snapshot(
        ProfileId::new(kernel::generate_id()),
        a.id().clone(),
        v + 10,
    );
    assert!(!db
        .profile_read_model()
        .update_content(&mut c, &missing, a.id())
        .await
        .unwrap());
    let wrong = snapshot(p.id().clone(), b.id().clone(), v + 10);
    assert!(!db
        .profile_read_model()
        .update_content(&mut c, &wrong, b.id())
        .await
        .unwrap());
    let newer = snapshot(p.id().clone(), b.id().clone(), v + 10);
    // Even a snapshot containing a different owner cannot SET ownership.
    assert!(db
        .profile_read_model()
        .update_content(&mut c, &newer, a.id())
        .await
        .unwrap());
    assert!(!db
        .profile_read_model()
        .update_content(&mut c, p, a.id())
        .await
        .unwrap());
    let (owner, kind, version): (i64, String, i64) =
        sqlx::query_as("SELECT account_id,owner_kind,version FROM profiles WHERE id=$1")
            .bind(p.id().as_ref())
            .fetch_one(&mut *c)
            .await
            .unwrap();
    assert_eq!(
        (owner, kind.as_str(), version),
        (*a.id().as_ref(), "organization", v + 10)
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE id=$1")
            .bind(missing.id().as_ref())
            .fetch_one(&mut *c)
            .await
            .unwrap(),
        0
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn set_writers_require_actual_account_gates_in_the_same_transaction() {
    let isolated = IsolatedDatabase::create().await;
    let db = &isolated.database;
    let (a, profiles) = fixture(db, 2).await;
    let (b, _) = fixture(db, 2).await;
    let p = &profiles[0];
    let v = *p.version().as_ref();
    let fresh = ProfileBuilder::new()
        .id(ProfileId::new(kernel::generate_id()))
        .account_id(a.id().clone())
        .build();
    let moved = ProfileBuilder::new()
        .id(p.id().clone())
        .account_id(b.id().clone())
        .version(kernel::prelude::entity::EventVersion::new(v + 1))
        .build();
    let mut plain = db.connection().await.unwrap();
    for transactional in [false, true] {
        let mut tx = db.get_transaction().await.unwrap();
        let c = if transactional {
            tx.connection()
        } else {
            &mut plain
        };
        assert!(db.profile_read_model().create(c, &fresh).await.is_err());
        assert!(db.profile_projection_writer().upsert(c, p).await.is_err());
        assert!(db.profile_read_model().update(c, &moved).await.is_err());
        assert!(PostgresHomeLinkStore
            .delete_owned_profiles(c, a.id())
            .await
            .is_err());
    }
    // A Profile gate, an unrelated Account gate, or another connection's gate is not proof.
    let mut holder = gated(db, p).await;
    let mut other = db.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_profiles(other.connection(), &[ProfileId::new(kernel::generate_id())])
        .await
        .unwrap();
    assert!(db
        .profile_projection_writer()
        .upsert(other.connection(), p)
        .await
        .is_err());
    drop(other);
    // Successful gated insert/upsert and owner update check BOTH source and destination.
    db.profile_read_model()
        .create(holder.connection(), &fresh)
        .await
        .unwrap();
    db.profile_projection_writer()
        .upsert(holder.connection(), p)
        .await
        .unwrap();
    assert!(db
        .profile_read_model()
        .update(holder.connection(), &moved)
        .await
        .is_err());
    drop(holder);
    let mut tx = db.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_accounts(tx.connection(), &[a.id().clone(), b.id().clone()])
        .await
        .unwrap();
    PostgresHomeLinkStore
        .lock_profiles(tx.connection(), std::slice::from_ref(p.id()))
        .await
        .unwrap();
    db.profile_read_model()
        .update(tx.connection(), &moved)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_accounts(tx.connection(), std::slice::from_ref(a.id()))
        .await
        .unwrap();
    // Target gate alone is insufficient for an upsert moving the current B-owned row to A.
    assert!(db
        .profile_projection_writer()
        .upsert(tx.connection(), p)
        .await
        .is_err());
    PostgresHomeLinkStore
        .delete_owned_profiles(tx.connection(), a.id())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut fresh_tx = db.get_transaction().await.unwrap();
    assert!(PostgresHomeLinkStore
        .delete_owned_profiles(fresh_tx.connection(), a.id())
        .await
        .is_err());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn rolled_back_savepoint_forgets_account_gate_proof() {
    let isolated = IsolatedDatabase::create().await;
    let db = &isolated.database;
    let (a, _) = fixture(db, 1).await;
    let mut tx = db.get_transaction().await.unwrap();
    let savepoint = tx.savepoint().await.unwrap();
    PostgresHomeLinkStore
        .lock_accounts(tx.connection(), std::slice::from_ref(a.id()))
        .await
        .unwrap();
    savepoint.rollback(tx.connection()).await.unwrap();
    assert!(PostgresHomeLinkStore
        .delete_owned_profiles(tx.connection(), a.id())
        .await
        .is_err());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn profile_order_guards_reject_each_inversion_and_allow_reentry() {
    let isolated = IsolatedDatabase::create().await;
    let db = &isolated.database;
    let low = ProfileId::new(kernel::generate_id());
    let high = ProfileId::new(kernel::generate_id());
    let mut descending = db.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_profiles(descending.connection(), std::slice::from_ref(&high))
        .await
        .unwrap();
    let error = PostgresHomeLinkStore
        .lock_profiles(descending.connection(), std::slice::from_ref(&low))
        .await
        .unwrap_err();
    assert!(matches!(error.current_context(), KernelError::Concurrency));
    drop(descending);
    let mut relationship = db.get_transaction().await.unwrap();
    PostgresHomeLinkStore
        .lock_profiles(relationship.connection(), std::slice::from_ref(&low))
        .await
        .unwrap();
    PostgresProfileRelationshipLock
        .lock_profiles(relationship.connection(), std::slice::from_ref(&low))
        .await
        .unwrap();
    let error = PostgresHomeLinkStore
        .lock_profiles(relationship.connection(), std::slice::from_ref(&high))
        .await
        .unwrap_err();
    assert!(matches!(error.current_context(), KernelError::Concurrency));
    PostgresHomeLinkStore
        .lock_profiles(relationship.connection(), std::slice::from_ref(&low))
        .await
        .unwrap();
}

/// Review guard for raw SQL bypasses. Scans all driver runtime Rust sources (not fixtures),
/// normalizes whitespace/case and lists EVERY Profile write, including the harmless ones.
#[test]
fn profile_write_statement_inventory_matches_allowlist() {
    fn scan(dir: &std::path::Path, root: &std::path::Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_str().unwrap();
            if name.contains("test") {
                continue;
            }
            if path.is_dir() {
                scan(&path, root, found);
                continue;
            }
            if path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).unwrap();
            let src = src.split("#[cfg(test)]").next().unwrap();
            let text = src
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase();
            for verb in [
                "insert into profiles ",
                "delete from profiles ",
                "update profiles ",
            ] {
                for (offset, _) in text.match_indices(verb) {
                    let rest = &text[offset + verb.len()..];
                    let kind = if verb.starts_with("update") {
                        if rest.split("where").next().unwrap().contains("account_id =") {
                            "owner-update"
                        } else {
                            "content-update"
                        }
                    } else if verb.starts_with("insert") {
                        "insert/upsert"
                    } else {
                        "delete"
                    };
                    found.push(format!(
                        "{}:{kind}",
                        path.strip_prefix(root).unwrap().display()
                    ));
                }
            }
        }
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut actual = vec![];
    scan(&root, &root, &mut actual);
    actual.sort();
    assert_eq!(
        actual,
        [
            "database/postgres/home_link.rs:delete",
            "database/postgres/profile.rs:content-update",
            "database/postgres/profile.rs:content-update",
            "database/postgres/profile.rs:insert/upsert",
            "database/postgres/profile.rs:owner-update",
            "database/postgres/projection.rs:insert/upsert",
            "profile_cutover_recovery.rs:content-update",
        ]
    );
}
