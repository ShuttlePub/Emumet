//! Cases 16-19: relink isolation, transfer, moderation/deactivation, projector replays.
use super::db::eventually;
use super::delivery::expedite;
use super::receiver::Behavior;
use super::world::*;
use serde_json::json;
use std::time::Duration;

fn short() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case16_relink_never_leaks_old_jobs_or_quarantine_to_the_new_home() {
    let w = World::setup().await;
    let a = w.linked("c16", 0).await;
    // One quarantined body (permanent failure) and one waiting job, both for the OLD home.
    w.receiver.push(Behavior::Status(400));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("old-quarantine"))
            .await
            .status(),
        202
    );
    let db = &w.db;
    eventually("quarantined", Duration::from_secs(30), || async {
        db.count("post_relay_dead_letters").await == 1
    })
    .await;
    w.receiver.set_default(Behavior::Status(503));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("old-job"))
            .await
            .status(),
        202
    );
    w.receiver.wait_attempts(2, Duration::from_secs(30)).await;
    let (status, _) = w.server.unlink(&a.user.jwt, &a.profile, &a.link_id).await;
    assert_eq!(status, 204);
    // Cancellation is committed with the unlink; the physical purge is a bounded maintenance step.
    eventually(
        "old job and quarantine purged",
        Duration::from_secs(15),
        || async {
            db.count("post_relay_jobs").await == 0
                && db.count("post_relay_bodies").await == 0
                && db.count("post_relay_dead_letters").await == 0
        },
    )
    .await;
    // Re-link to ANOTHER home: a new link id; only traffic received afterwards goes there.
    w.receiver.set_default(Behavior::Accept);
    let new_link = w.register_at(&a.user, &a.profile, 1, &w.receiver2).await;
    assert_ne!(new_link, a.link_id, "link ids are never reused");
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("new-home"))
            .await
            .status(),
        202
    );
    let got = w.receiver2.wait_accepted(1, Duration::from_secs(30)).await;
    assert_eq!(got[0].envelope["recipient"]["link_id"], new_link);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(
        w.receiver2.accepted().len(),
        1,
        "no old job or quarantined body reached the new home"
    );
    assert_eq!(
        w.receiver.accepted().len(),
        0,
        "nothing reached the old home after the unlink"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case17_transfer_keeps_the_link_and_only_the_new_owner_can_operate_it() {
    let w = World::setup().await;
    let a = w.linked("c17a", 0).await;
    let b = w.user("c17b").await;
    let _ = w
        .server
        .create_account(&b.jwt, &format!("t1_c17b_{}", short()))
        .await;
    let (status, org) = w
        .server
        .api(
            "POST",
            "/api/v1/organizations",
            &b.jwt,
            Some(json!({"name": format!("t1org{}", short())})),
        )
        .await;
    assert_eq!(status, 201, "{org}");
    let org_id = org["id"].as_str().unwrap().to_string();
    // The requester must be an active member of the organization.
    let (status, body) = w
        .server
        .api(
            "POST",
            &format!("/api/v1/organizations/{org_id}/invites"),
            &b.jwt,
            Some(json!({"account_id": a.account, "role": "member"})),
        )
        .await;
    assert_eq!(status, 204, "{body}");
    let (status, body) = w
        .server
        .api(
            "POST",
            &format!(
                "/api/v1/organizations/{org_id}/invites/{}/accept",
                a.account
            ),
            &a.user.jwt,
            None,
        )
        .await;
    assert_eq!(status, 204, "{body}");
    // A job is waiting when the transfer happens.
    w.receiver.set_default(Behavior::Status(503));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("before transfer"))
            .await
            .status(),
        202
    );
    w.receiver.wait_attempts(1, Duration::from_secs(30)).await;
    let (status, req) = w
        .server
        .api(
            "POST",
            &format!("/api/v1/profiles/{}/transfer-requests", a.profile),
            &a.user.jwt,
            Some(json!({"org_account_nanoid": org_id})),
        )
        .await;
    assert_eq!(status, 201, "{req}");
    let (status, body) = w
        .server
        .api(
            "POST",
            &format!(
                "/api/v1/profile-transfer-requests/{}/accept",
                req["nanoid"].as_str().unwrap()
            ),
            &b.jwt,
            None,
        )
        .await;
    assert_eq!(status, 204, "{body}");
    // Same link id/origin/generation; the former owner is locked out, the new owner sees it.
    let (status, links) = w
        .server
        .api_org(
            "GET",
            &format!("/api/v1/profiles/{}/links", a.profile),
            &b.jwt,
            Some(&org_id),
            None,
        )
        .await;
    assert_eq!(status, 200, "{links}");
    assert_eq!(links["items"][0]["id"], a.link_id);
    assert_eq!(links["items"][0]["generation"], 1);
    assert_eq!(links["items"][0]["state"], "active");
    let (status, _) = w.server.unlink(&a.user.jwt, &a.profile, &a.link_id).await;
    assert!(
        status == 403 || status == 404,
        "former owner must not unlink: {status}"
    );
    let (status, _) = w
        .server
        .register_link(&a.user.jwt, &a.profile, &home(3))
        .await;
    assert!(
        status == 403 || status == 404,
        "former owner must not register: {status}"
    );
    // The same link keeps delivering to the same home.
    w.receiver.set_default(Behavior::Accept);
    expedite(&w).await;
    let got = w.receiver.wait_accepted(1, Duration::from_secs(40)).await;
    assert_eq!(got[0].envelope["recipient"]["link_id"], a.link_id);
    assert_eq!(got[0].envelope["recipient"]["link_generation"], 1);
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("after transfer"))
            .await
            .status(),
        202
    );
    let got = w.receiver.wait_accepted(2, Duration::from_secs(40)).await;
    assert_eq!(got[1].envelope["recipient"]["link_id"], a.link_id);
    let (status, body) = w
        .server
        .api_org(
            "DELETE",
            &format!("/api/v1/profiles/{}/links/{}", a.profile, a.link_id),
            &b.jwt,
            Some(&org_id),
            None,
        )
        .await;
    assert_eq!(status, 204, "the new owner can unlink: {body}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case18_suspend_ban_hold_without_extending_and_deactivation_revokes_even_if_the_profile_row_stays(
) {
    let w = World::setup().await;
    let admin = w.user("c18admin").await;
    // The admin needs an AuthAccount row: any authenticated call creates it.
    let _ = w
        .server
        .create_account(&admin.jwt, &format!("t1_adm_{}", short()))
        .await;
    w.make_admin(&admin).await;
    for (kind, undo) in [("suspend", "unsuspend"), ("ban", "unban")] {
        let a = w
            .linked(&format!("c18{kind}"), if kind == "ban" { 1 } else { 0 })
            .await;
        let receiver = if kind == "ban" {
            &w.receiver2
        } else {
            &w.receiver
        };
        if kind == "ban" {
            w.db.point_link_at(&a.link_id, &w.receiver2.origin).await;
        }
        receiver.set_default(Behavior::Status(503));
        assert_eq!(
            w.peer
                .deliver_json(&w.server, &a.profile, &w.peer.note(kind))
                .await
                .status(),
            202
        );
        receiver.wait_attempts(1, Duration::from_secs(30)).await;
        let expires: String = sqlx::query_scalar(&format!(
            "SELECT automatic_expires_at::text FROM post_relay_bodies WHERE profile_id={}",
            a.profile_id
        ))
        .fetch_one(&w.db.iso.pool)
        .await
        .unwrap();
        let (status, body) = w
            .server
            .api(
                "POST",
                &format!("/api/v1/admin/accounts/{}/{kind}", a.account),
                &admin.jwt,
                Some(json!({"reason": "t1 moderation"})),
            )
            .await;
        assert!(status == 204 || status == 200, "{kind}: {status} {body}");
        tokio::time::sleep(Duration::from_secs(2)).await; // owner status is a projected read model
        let attempts = receiver.attempts().len();
        expedite(&w).await;
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(
            receiver.attempts().len(),
            attempts,
            "{kind}: a held job must not be sent"
        );
        assert_eq!(w.db.i64(&format!("SELECT count(*) FROM profile_links WHERE id='{}' AND state='active' AND generation=1", a.link_id)).await, 1, "{kind}: the link is kept");
        let r = w
            .peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("while suspended"))
            .await;
        assert_eq!(
            r.status(),
            404,
            "{kind}: a non-active owner receives nothing new"
        );
        receiver.set_default(Behavior::Accept);
        let (status, body) = w
            .server
            .api(
                "POST",
                &format!("/api/v1/admin/accounts/{}/{undo}", a.account),
                &admin.jwt,
                None,
            )
            .await;
        assert!(status == 204 || status == 200, "{undo}: {status} {body}");
        tokio::time::sleep(Duration::from_secs(2)).await;
        expedite(&w).await;
        let got = receiver.wait_accepted(1, Duration::from_secs(40)).await;
        assert_eq!(got[0].envelope["recipient"]["link_id"], a.link_id);
        // The 72 h deadline never moved while held.
        let still: String = got[0].envelope["received_at"].as_str().unwrap().to_string();
        assert!(!still.is_empty() && !expires.is_empty());
    }
    // Deactivation revokes at the command, even when the Profile row is NOT deleted afterwards.
    w.db.exec("CREATE FUNCTION t1_keep() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NULL; END $$").await;
    w.db.exec(
        "CREATE TRIGGER t1_keep BEFORE DELETE ON profiles FOR EACH ROW EXECUTE FUNCTION t1_keep()",
    )
    .await;
    let a = w.linked("c18deact", 2).await;
    w.receiver.set_default(Behavior::Status(503));
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("deact"))
            .await
            .status(),
        202
    );
    let (status, body) = w
        .server
        .api(
            "DELETE",
            &format!("/api/v1/accounts/{}", a.account),
            &a.user.jwt,
            None,
        )
        .await;
    assert!(status == 204 || status == 200, "{status} {body}");
    let (state, generation): (String, i64) = sqlx::query_as(&format!(
        "SELECT state, generation FROM profile_links WHERE id='{}'",
        a.link_id
    ))
    .fetch_one(&w.db.iso.pool)
    .await
    .unwrap();
    assert_eq!((state.as_str(), generation), ("unlinked", 2));
    let db = &w.db;
    eventually("body purged", Duration::from_secs(15), || async {
        db.count("post_relay_bodies").await == 0
    })
    .await;
    assert_eq!(
        w.db.i64(&format!(
            "SELECT count(*) FROM profiles WHERE id={}",
            a.profile_id
        ))
        .await,
        1,
        "the Profile row stayed (logical-delete model)"
    );
    // Reactivation never revives the old link; the owner registers a new one.
    let (status, body) = w
        .server
        .api(
            "POST",
            &format!("/api/v1/accounts/{}/reactivate", a.account),
            &a.user.jwt,
            None,
        )
        .await;
    assert!(
        status == 200 || status == 204,
        "reactivate: {status} {body}"
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        w.db.i64(&format!(
            "SELECT count(*) FROM profile_links WHERE id='{}' AND state='unlinked'",
            a.link_id
        ))
        .await,
        1
    );
    w.receiver.set_default(Behavior::Accept);
    let before = w.receiver.accepted().len();
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("after reactivation"))
            .await
            .status(),
        202
    );
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(
        w.receiver.accepted().len(),
        before,
        "an unlinked reactivated Profile forwards nothing"
    );
    let new_link = w.register(&a.user, &a.profile, 2).await;
    assert_ne!(new_link, a.link_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case19_deactivate_and_reactivate_in_one_batch_and_old_event_replay_keep_a_new_link() {
    let w = World::setup().await;
    let a = w.linked("c19", 0).await;
    let account_id: i64 = sqlx::query_scalar("SELECT id FROM accounts WHERE nanoid=$1")
        .bind(&a.account)
        .fetch_one(&w.db.iso.pool)
        .await
        .unwrap();
    let version: i64 =
        w.db.i64(&format!(
            "SELECT max(version) FROM account_events WHERE id={account_id}"
        ))
        .await;
    // Deactivated then Reactivated committed together (one projector batch), written by an "old
    // producer" that has no link hook.
    let mut tx = w.db.iso.pool.begin().await.unwrap();
    for (i, (name, kind)) in [
        ("account_deactivated", "Deactivated"),
        ("account_reactivated", "Reactivated"),
    ]
    .into_iter()
    .enumerate()
    {
        sqlx::query("INSERT INTO account_events(version,id,event_name,data) VALUES($1,$2,$3,$4)")
            .bind(version + 1 + i as i64)
            .bind(account_id)
            .bind(name)
            .bind(json!({"type": kind}))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    let db = &w.db;
    let link = a.link_id.clone();
    eventually(
        "the batch revoked the old link",
        Duration::from_secs(15),
        || async {
            db.i64(&format!(
                "SELECT count(*) FROM profile_links WHERE id='{link}' AND state='unlinked'"
            ))
            .await
                == 1
        },
    )
    .await;
    assert_eq!(
        w.db.i64(&format!(
            "SELECT count(*) FROM profile_links WHERE id='{}' AND state='active'",
            a.link_id
        ))
        .await,
        0,
        "the old link was not revived"
    );
    // The owner registers a NEW link, then the whole event history is replayed.
    let new_link = w.register(&a.user, &a.profile, 1).await;
    assert_ne!(new_link, a.link_id);
    w.db.exec("UPDATE projection_checkpoints SET last_seq = 0")
        .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    let (state, generation): (String, i64) = sqlx::query_as(&format!(
        "SELECT state, generation FROM profile_links WHERE id='{new_link}'"
    ))
    .fetch_one(&w.db.iso.pool)
    .await
    .unwrap();
    assert_eq!(
        (state.as_str(), generation),
        ("active", 1),
        "an old deactivation replay must not unlink the new link"
    );
    assert_eq!(
        w.peer
            .deliver_json(&w.server, &a.profile, &w.peer.note("after replay"))
            .await
            .status(),
        202
    );
    let got = w.receiver.wait_accepted(1, Duration::from_secs(40)).await;
    assert_eq!(got[0].envelope["recipient"]["link_id"], new_link);
}
