//! Cases 13-15: P0-P4 stop points x three revokers x automatic/operator delivery, stalled HTTP,
//! absolute send deadline. Real Ory auth, real server API/projector as revokers, real DB.
use super::db::eventually;
use super::worker::TestModule;
use super::world::*;
use std::time::{Duration, Instant};

/// 0 = unlink API, 1 = Account deactivation command (API), 2 = old-producer deactivation event
/// applied by the server's Account projector.
async fn revoke(w: &World, a: &Linked, path: u8) -> Duration {
    let started = Instant::now();
    match path {
        0 => {
            let (status, body) = w.server.unlink(&a.user.jwt, &a.profile, &a.link_id).await;
            assert_eq!(status, 204, "{body}");
        }
        1 => {
            let (status, body) = w.server.api("DELETE", &format!("/api/v1/accounts/{}", a.account), &a.user.jwt, None).await;
            assert!(status == 204 || status == 200, "deactivate: {status} {body}");
        }
        _ => {
            let account_id: i64 = sqlx::query_scalar("SELECT id FROM accounts WHERE nanoid=$1").bind(&a.account).fetch_one(&w.db.iso.pool).await.unwrap();
            let version: i64 = w.db.i64(&format!("SELECT max(version) FROM account_events WHERE id={account_id}")).await;
            sqlx::query("INSERT INTO account_events(version,id,event_name,data) VALUES($1,$2,'account_deactivated',$3)")
                .bind(version + 1)
                .bind(account_id)
                .bind(serde_json::json!({"type":"Deactivated"}))
                .execute(&w.db.iso.pool)
                .await
                .unwrap();
            let link = a.link_id.clone();
            let db = &w.db;
            eventually("projector revoked the link", Duration::from_secs(10), || async { db.i64(&format!("SELECT count(*) FROM profile_links WHERE id='{link}' AND state='unlinked'")).await == 1 }).await;
        }
    }
    started.elapsed()
}

async fn admit(w: &World, a: &Linked, label: &str) -> String {
    let r = w.peer.deliver_json(&w.server, &a.profile, &w.peer.note(label)).await;
    assert_eq!(r.status(), 202);
    sqlx::query_scalar("SELECT receipt FROM post_relay_jobs").fetch_one(&w.db.iso.pool).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case13_p0_p1_p2_p4_x_three_revokers_x_auto_and_operator() {
    let w = World::setup_with(Options { sender: false, ..Default::default() }).await;
    let module = TestModule::new(w.db.iso.database.clone(), &w.server.dir, &w.receiver.origin);
    let mut combo = 0u32;
    for manual in [false, true] {
        for path in 0..3u8 {
            for point in [0u8, 1, 2, 4] {
                combo += 1;
                let a = w.linked(&format!("c13x{combo}"), (combo % 8) as u8).await;
                let receipt = admit(&w, &a, &format!("race {combo}")).await;
                if manual {
                    module.stage_manual(a.profile_id, &receipt, &format!("t1-op-{combo}")).await;
                }
                let before = w.receiver.attempts().len();
                let (worker, pause) = module.paused_worker(point);
                let lease = worker.claim(1).await.unwrap().remove(0);
                let deliver = { let worker = worker.clone(); tokio::spawn(async move { worker.deliver(lease).await }) };
                tokio::time::timeout(Duration::from_secs(10), pause.reached.notified())
                    .await
                    .unwrap_or_else(|_| panic!("stop point P{point} not reached (manual={manual} path={path})"));
                let elapsed = if point == 1 {
                    // P1: the permission transaction holds the common gate until it commits.
                    let revoker = tokio::spawn({
                        let (server_base, jwt, profile, link, account) = (w.server.base.clone(), a.user.jwt.clone(), a.profile.clone(), a.link_id.clone(), a.account.clone());
                        async move {
                            let c = super::emumet::client();
                            match path {
                                0 => c.delete(format!("{server_base}/api/v1/profiles/{profile}/links/{link}")).bearer_auth(jwt).send().await.unwrap().status().as_u16(),
                                1 => c.delete(format!("{server_base}/api/v1/accounts/{account}")).bearer_auth(jwt).send().await.unwrap().status().as_u16(),
                                _ => 0,
                            }
                        }
                    });
                    if path == 2 {
                        revoker.abort();
                        let started = Instant::now();
                        // the projector path: the event is applied only after the permit commits
                        let resume = pause.clone();
                        let r = tokio::spawn({ let w_pool = w.db.iso.pool.clone(); let account = a.account.clone(); async move {
                            let account_id: i64 = sqlx::query_scalar("SELECT id FROM accounts WHERE nanoid=$1").bind(&account).fetch_one(&w_pool).await.unwrap();
                            let version: i64 = sqlx::query_scalar("SELECT max(version) FROM account_events WHERE id=$1").bind(account_id).fetch_one(&w_pool).await.unwrap();
                            sqlx::query("INSERT INTO account_events(version,id,event_name,data) VALUES($1,$2,'account_deactivated',$3)").bind(version + 1).bind(account_id).bind(serde_json::json!({"type":"Deactivated"})).execute(&w_pool).await.unwrap();
                        }});
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        resume.resume.notify_one();
                        r.await.unwrap();
                        let link = a.link_id.clone();
                        let db = &w.db;
                        eventually("projector revoked the link", Duration::from_secs(10), || async { db.i64(&format!("SELECT count(*) FROM profile_links WHERE id='{link}' AND state='unlinked'")).await == 1 }).await;
                        started.elapsed()
                    } else {
                        let started = Instant::now();
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        assert!(!revoker.is_finished(), "unlink/deactivation must wait for the permission gate at P1");
                        pause.resume.notify_one();
                        let status = tokio::time::timeout(Duration::from_secs(8), revoker).await.expect("revoker must finish once the permit commits").unwrap();
                        assert!(status == 204 || status == 200, "revoker status {status}");
                        started.elapsed()
                    }
                } else {
                    let e = tokio::time::timeout(Duration::from_secs(8), revoke(&w, &a, path)).await.expect("revoker must not wait for the paused worker");
                    pause.resume.notify_one();
                    e
                };
                eprintln!("T1_REVOKE manual={manual} path={path} point=P{point} ms={:.0}", elapsed.as_secs_f64() * 1000.0);
                deliver.await.unwrap().unwrap();
                worker.maintain().await.unwrap();
                let db = &w.db;
                eventually("no body, job or quarantine resurrected", Duration::from_secs(10), || async {
                    db.count("post_relay_bodies").await == 0 && db.count("post_relay_jobs").await == 0 && db.count("post_relay_dead_letters").await == 0
                })
                .await;
                let sent = w.receiver.attempts().len() - before;
                assert_eq!(sent, usize::from(point != 0), "manual={manual} path={path} P{point}: sends");
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case14_stalled_http_does_not_delay_unlink_deactivation_or_projector() {
    let w = World::setup().await;
    w.receiver.set_default(super::receiver::Behavior::Stall);
    for path in 0..3u8 {
        let a = w.linked(&format!("c14x{path}"), path).await;
        let before = w.receiver.attempts().len();
        admit_any(&w, &a).await;
        w.receiver.wait_attempts(before + 1, Duration::from_secs(30)).await;
        // The request is in flight (stalled for 3 s now): no transaction has been idle for more than
        // 2 s (the 1 s maintenance and the projector open short transactions of their own), and no
        // advisory lock is held.
        tokio::time::sleep(Duration::from_secs(3)).await;
        let idle = w.db.i64("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND state LIKE 'idle in transaction%' AND clock_timestamp()-xact_start > interval '2 seconds'").await;
        // Transaction-scoped gates are taken by the 1 s maintenance and the projector for
        // milliseconds; an HTTP-held gate would be present in EVERY sample.
        let mut advisory = i64::MAX;
        for _ in 0..30 {
            advisory = advisory.min(w.db.i64("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND granted").await);
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        if (idle, advisory) != (0, 0) {
            let rows: Vec<(i32, String, String)> = sqlx::query_as("SELECT l.pid, a.state, left(a.query,200)||' | '||l.classid||'/'||l.objid||' xact='||coalesce(a.xact_start::text,'none') FROM pg_locks l JOIN pg_stat_activity a USING(pid) WHERE l.locktype='advisory' AND l.granted").fetch_all(&w.db.iso.pool).await.unwrap();
            panic!("HTTP must hold no DB transaction/lock: idle={idle} advisory={advisory} {rows:?}");
        }
        let took = revoke(&w, &a, path).await;
        assert!(took < Duration::from_secs(3), "path {path} waited for the stalled HTTP response: {took:?}");
        let db = &w.db;
        eventually("job and body cancelled", Duration::from_secs(10), || async { db.count("post_relay_jobs").await == 0 && db.count("post_relay_bodies").await == 0 }).await;
        eprintln!("T1_SILENT_REVOKE path={path} ms={:.0}", took.as_secs_f64() * 1000.0);
    }
}

async fn admit_any(w: &World, a: &Linked) {
    let r = w.peer.deliver_json(&w.server, &a.profile, &w.peer.note("stalled")).await;
    assert_eq!(r.status(), 202);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs the T1 stack: bash e2e/post-relay/run.sh"]
async fn case15_worker_past_its_absolute_send_deadline_does_not_send() {
    let w = World::setup_with(Options { sender: false, ..Default::default() }).await;
    let module = TestModule::new(w.db.iso.database.clone(), &w.server.dir, &w.receiver.origin);
    let a = w.linked("c15", 0).await;
    admit(&w, &a, "late worker").await;
    let (worker, pause) = module.paused_worker(2);
    let lease = worker.claim(1).await.unwrap().remove(0);
    let deliver = { let worker = worker.clone(); tokio::spawn(async move { worker.deliver(lease).await }) };
    tokio::time::timeout(Duration::from_secs(10), pause.reached.notified()).await.expect("P2 reached");
    // P2 = after the authorization commit, before HTTP. The 10 s absolute window elapses here.
    tokio::time::sleep(Duration::from_secs(11)).await;
    pause.resume.notify_one();
    deliver.await.unwrap().unwrap();
    assert_eq!(w.receiver.attempts().len(), 0, "an expired authorization must never send");
    assert_eq!(w.db.count("post_relay_bodies").await, 1, "the body is kept for the next fenced attempt");
}
