//! I1 HTTP admission with real PostgreSQL. No DB skipping counts as a pass.
use crate::route::profile_graph_test_support::{
    peer_private_pem, GraphWorld, LocalProfile, RemotePeer,
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
    response::Response,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use kernel::interfaces::{
    database::{DependOnTransactionManager, TransactionManager},
    home_link::{DependOnHomeLinkStore, HomeLinkStore},
    http_signing::{HttpSigner, HttpSigningRequest},
    post_relay::{CapacityReport, DependOnPostRelayStore, PostRelayStore},
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, time::SystemTime};
use time::OffsetDateTime;
use tower::ServiceExt;

async fn world() -> GraphWorld {
    let mut w = GraphWorld::new().await;
    w.base_url = "https://emumet.example".into();
    w.host_header = "emumet.example".into();
    w.module = w
        .module
        .clone()
        .with_post_relay_inbox(true)
        .with_public_base_url(&w.base_url);
    capacity(&w).await;
    w
}
async fn capacity(w: &GraphWorld) {
    let m = w.module.clone();
    w.module
        .transaction_manager()
        .transaction(move |c| {
            Box::pin(async move {
                m.post_relay_store()
                    .report_capacity(
                        c,
                        &CapacityReport {
                            free_bytes: 50 << 30,
                            total_bytes: 100 << 30,
                            sampled_at: OffsetDateTime::now_utc(),
                        },
                    )
                    .await
            })
        })
        .await
        .unwrap();
}
async fn link(w: &GraphWorld, p: &LocalProfile) {
    let m = w.module.clone();
    let p = p.clone();
    w.module
        .transaction_manager()
        .transaction(move |c| {
            Box::pin(async move {
                let store = m.home_link_store();
                let (id, a) = store.owner_candidate(c, &p.nanoid).await?;
                store.lock_accounts(c, std::slice::from_ref(&a)).await?;
                store.lock_profiles(c, std::slice::from_ref(&id)).await?;
                let owner = store.owner(c, &id, &a).await?;
                store
                    .register(c, &owner, &p.actor_url, "https://home.example")
                    .await?;
                Ok(())
            })
        })
        .await
        .unwrap();
}
async fn request(
    w: &GraphWorld,
    p: &LocalProfile,
    peer: &RemotePeer,
    raw: &[u8],
    ct: &str,
) -> Request<Body> {
    let target = format!("/ap/profiles/{}/inbox", p.nanoid);
    let headers = HashMap::from([
        ("host".into(), w.host_header.clone()),
        ("date".into(), httpdate::fmt_http_date(SystemTime::now())),
        (
            "digest".into(),
            format!("SHA-256={}", STANDARD.encode(Sha256::digest(raw))),
        ),
        ("content-type".into(), ct.into()),
    ]);
    let signed = driver::http_signing::HttpSignerImpl
        .sign(
            &HttpSigningRequest {
                method: "POST".into(),
                url: format!("{}{target}", w.base_url),
                headers: headers.clone(),
                body: Some(raw.to_vec()),
            },
            peer_private_pem(),
            &peer.key_id,
            &kernel::interfaces::crypto::SigningAlgorithm::Rsa2048,
        )
        .await
        .unwrap();
    let mut req = Request::builder().method("POST").uri(target);
    for (n, v) in headers.iter().chain(signed.cavage_headers.iter()) {
        req = req.header(n, v);
    }
    req.body(Body::from(raw.to_vec())).unwrap()
}
async fn post(w: &GraphWorld, p: &LocalProfile, peer: &RemotePeer, v: &Value) -> Response {
    w.inbox_router()
        .oneshot(
            request(
                w,
                p,
                peer,
                &serde_json::to_vec(v).unwrap(),
                "application/activity+json",
            )
            .await,
        )
        .await
        .unwrap()
}
async fn count(w: &GraphWorld, table: &str) -> i64 {
    sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
        .fetch_one(w.pool())
        .await
        .unwrap()
}
async fn envelopes(w: &GraphWorld) -> Vec<Value> {
    let bytes: Vec<Vec<u8>> =
        sqlx::query_scalar("SELECT envelope FROM post_relay_bodies ORDER BY receipt")
            .fetch_all(w.pool())
            .await
            .unwrap();
    bytes
        .iter()
        .map(|b| serde_json::from_slice(b).unwrap())
        .collect()
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn raw_variants_preserve_bytes_and_verify_envelope_schema() {
    let w = world().await;
    let p = w.actor_profile().await;
    link(&w, &p).await;
    let peer = RemotePeer::new();
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../docs/contracts/post-relay-v1.schema.json"
    ))
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    for kind in [
        "Create",
        "Update",
        "Delete",
        "Like",
        "Announce",
        "https://extension.example/EmojiReact",
    ] {
        let raw = format!(
            r#"{{ "type":"{kind}", "actor":{{"id":"{}"}}, "to":"Public", "object":[{{"content":"日本語"}},42], "unknown": true }}"#,
            peer.actor_url
        );
        let response=w.inbox_router().oneshot(request(&w,&p,&peer,raw.as_bytes(),"Application/LD+JSON; profile=\"https://www.w3.org/ns/activitystreams\"; charset=UTF-8").await).await.unwrap();
        assert_eq!(response.status(), StatusCode::ACCEPTED, "{kind}");
        let all = envelopes(&w).await;
        let e = all
            .iter()
            .find(|e| {
                STANDARD
                    .decode(e["activity"]["raw_body"].as_str().unwrap())
                    .unwrap()
                    == raw.as_bytes()
            })
            .unwrap();
        assert!(
            validator.is_valid(e),
            "{}",
            validator
                .iter_errors(e)
                .map(|e| e.to_string())
                .collect::<Vec<_>>()
                .join(";")
        );
        assert_eq!(e["activity"]["id"], Value::Null);
        assert_eq!(e["verification"]["key_id"], peer.key_id);
        assert_eq!(e["verification"]["signer_actor"], peer.actor_url);
    }
    assert_eq!(count(&w, "post_relay_jobs").await, 6);
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn receive_and_early_undo_rollback_retry_duplicate_and_other_profile_are_atomic() {
    let mut w = world().await;
    let a = w.actor_profile().await;
    let b = w.actor_profile().await;
    link(&w, &a).await;
    link(&w, &b).await;
    let peer = RemotePeer::new();
    let follow = peer.activity_id("late");
    let undo = peer.undo(&peer.activity_id("u"), json!(follow));
    sqlx::raw_sql("CREATE FUNCTION i1_fail() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected admission failure'; END $$; CREATE TRIGGER i1_fail BEFORE INSERT ON post_relay_bodies FOR EACH ROW EXECUTE FUNCTION i1_fail();").execute(w.pool()).await.unwrap();
    assert_eq!(
        post(&w, &a, &peer, &undo).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(count(&w, "profile_inbound_activities").await, 0);
    assert_eq!(count(&w, "post_relay_jobs").await, 0);
    let used: i64 = sqlx::query_scalar("SELECT body_count FROM post_relay_quota")
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(used, 0);
    sqlx::raw_sql("DROP TRIGGER i1_fail ON post_relay_bodies; DROP FUNCTION i1_fail();")
        .execute(w.pool())
        .await
        .unwrap();
    // Legacy receive record / guard exists but no relay: Duplicate must still enqueue.
    w.module = w.module.clone().with_post_relay_inbox(false);
    assert_eq!(
        post(&w, &a, &peer, &undo).await.status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(count(&w, "post_relay_jobs").await, 0);
    w.module = w.module.clone().with_post_relay_inbox(true);
    for p in [&a, &a, &b] {
        assert_eq!(
            post(&w, p, &peer, &undo).await.status(),
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(count(&w, "post_relay_jobs").await, 3); // lost committed 202 can duplicate, never disappear
    assert_eq!(
        post(&w, &a, &peer, &peer.follow(&follow, &a.actor_url))
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(count(&w, "profile_follows").await, 0); // late Follow cannot revive
    let observed:bool=sqlx::query_scalar("SELECT observed_relation FROM profile_inbound_activities WHERE receiver_profile_id=$1 AND activity_id=$2").bind(a.id.as_ref()).bind(&follow).fetch_one(w.pool()).await.unwrap();
    assert!(observed);
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn d63_known_unknown_mixed_arrays_and_early_undo_are_not_confused() {
    let w = world().await;
    let p = w.actor_profile().await;
    link(&w, &p).await;
    let peer = RemotePeer::new();
    let known = peer.activity_id("block");
    assert_eq!(
        post(
            &w,
            &p,
            &peer,
            &json!({"id":known,"type":["Block","https://ext.example/T"],"actor":peer.actor_url})
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    let unknown = peer.activity_id("unknown");
    let mut expected = 0;
    for kind in ["Undo", "Accept", "Reject"] {
        for (i, object) in [
            json!(known),
            json!({"type":"Block"}),
            json!(unknown),
            json!([]),
            json!([known, unknown]),
            Value::Null,
        ]
        .into_iter()
        .enumerate()
        {
            let v = json!({"id":peer.activity_id(&format!("{kind}-{i}")),"type":[kind,"https://ext.example/T"],"actor":peer.actor_url,"object":object});
            assert_eq!(
                post(&w, &p, &peer, &v).await.status(),
                StatusCode::ACCEPTED,
                "{kind}-{i}"
            );
            if i >= 2 {
                expected += 1;
            }
            assert_eq!(count(&w, "post_relay_jobs").await, expected, "{kind}-{i}");
        }
    }
    let late = peer.activity_id("embedded-late");
    let embedded = json!({"type":"Follow","id":late,"actor":peer.actor_url,"object":p.actor_url});
    assert_eq!(
        post(
            &w,
            &p,
            &peer,
            &peer.undo(&peer.activity_id("embedded"), embedded)
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(
        post(
            &w,
            &p,
            &peer,
            &peer.undo(&peer.activity_id("bare-later"), json!(late))
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(count(&w, "post_relay_jobs").await, expected + 1); // synthetic generation is not evidence
    let ambiguous = json!({"id":peer.activity_id("ambiguous"),"type":["Follow","Undo"],"actor":peer.actor_url,"object":p.actor_url});
    assert_eq!(
        post(&w, &p, &peer, &ambiguous).await.status(),
        StatusCode::BAD_REQUEST
    );
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn unlinked_is_body_free_overload_rolls_back_and_flag_defaults_off() {
    let mut w = world().await;
    let p = w.actor_profile().await;
    let peer = RemotePeer::new();
    let body = json!({"type":"Create","id":peer.activity_id("secret-id"),"actor":peer.actor_url,"object":{"content":"secret-body"}});
    w.module = w.module.clone().with_post_relay_inbox(false);
    assert_eq!(
        post(&w, &p, &peer, &body).await.status(),
        StatusCode::ACCEPTED
    );
    assert_eq!(count(&w, "post_relay_unlinked").await, 0);
    w.module = w.module.clone().with_post_relay_inbox(true);
    for _ in 0..3 {
        assert_eq!(
            post(&w, &p, &peer, &body).await.status(),
            StatusCode::ACCEPTED
        );
    }
    assert_eq!(count(&w, "post_relay_unlinked").await, 1);
    assert_eq!(count(&w, "post_relay_bodies").await, 0);
    assert_eq!(count(&w, "profile_inbound_activities").await, 0);
    let audit: Value =
        sqlx::query_scalar("SELECT row_to_json(u)::jsonb FROM post_relay_unlinked u")
            .fetch_one(w.pool())
            .await
            .unwrap();
    assert_eq!(audit["count"], 3);
    assert!(!audit.to_string().contains("secret"));
    link(&w, &p).await;
    sqlx::query("UPDATE post_relay_capacity SET sampled_at=NULL")
        .execute(w.pool())
        .await
        .unwrap();
    let undo = peer.undo(
        &peer.activity_id("overload-undo"),
        json!(peer.activity_id("overload-late")),
    );
    let r = post(&w, &p, &peer, &undo).await;
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.headers()["retry-after"], "1");
    assert_eq!(count(&w, "profile_inbound_activities").await, 0);
    capacity(&w).await;
    // First successful admission creates a delivery quota, then force the exact Profile bound.
    assert_eq!(
        post(&w, &p, &peer, &body).await.status(),
        StatusCode::ACCEPTED
    );
    sqlx::query("UPDATE post_relay_profile_quota SET body_count=2000 WHERE profile_id=$1")
        .bind(p.id.as_ref())
        .execute(w.pool())
        .await
        .unwrap();
    let r = post(&w, &p, &peer, &undo).await;
    assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(r.headers()["retry-after"], "1");
    assert_eq!(count(&w, "profile_inbound_activities").await, 0);
    sqlx::query("UPDATE post_relay_profile_quota SET body_count=1 WHERE profile_id=$1")
        .bind(p.id.as_ref())
        .execute(w.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE post_relay_quota SET body_count=100000")
        .execute(w.pool())
        .await
        .unwrap();
    let r = post(&w, &p, &peer, &undo).await;
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.headers()["retry-after"], "1");
    assert_eq!(count(&w, "profile_inbound_activities").await, 0);
}

#[derive(Clone, Default)]
struct IntakeMetrics(std::sync::Arc<std::sync::Mutex<HashMap<String, Vec<u64>>>>);
impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for IntakeMetrics {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        struct Fields<'a>(&'a IntakeMetrics);
        impl tracing::field::Visit for Fields<'_> {
            fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
            fn record_u64(&mut self, f: &tracing::field::Field, v: u64) {
                if [
                    "signature_us",
                    "read_us",
                    "admission_us",
                    "accepted_us",
                    "transaction_begin_us",
                    "inbox_receiver_pool_acquire_us",
                ]
                .contains(&f.name())
                {
                    self.0
                         .0
                        .lock()
                        .unwrap()
                        .entry(f.name().to_string())
                        .or_default()
                        .push(v);
                }
            }
        }
        event.record(&mut Fields(self));
    }
}
fn percentiles(v: &mut [u64]) -> Value {
    v.sort_unstable();
    if v.is_empty() {
        return Value::Null;
    }
    json!({"p50":v[(v.len()-1)/2],"p95":v[(v.len()-1)*95/100],"p99":v[(v.len()-1)*99/100],"max":v[v.len()-1]})
}
fn cpu_ticks() -> u64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap();
    let tail = s
        .rsplit_once(')')
        .unwrap()
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    tail[11].parse::<u64>().unwrap() + tail[12].parse::<u64>().unwrap()
}
fn rss_kib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap()
        .lines()
        .find(|l| l.starts_with("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse()
        .unwrap()
}
/// Explicit release probe, never an implicit DB skip. Sender signing/setup is outside timing.
#[ignore = "run with DATABASE_URL, --release, --ignored --nocapture; writes no shared tables"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn measure_i1_intake_shared_quota_and_hour_bucket() {
    use tracing_subscriber::prelude::*;
    assert!(std::env::var("DATABASE_URL").is_ok(), "real DB required");
    let metrics = IntakeMetrics::default();
    tracing::subscriber::set_global_default(tracing_subscriber::registry().with(metrics.clone()))
        .unwrap();
    let total = 1024usize;
    let burst = 16usize;
    for linked in [false, true] {
        for profiles in [1usize, 8] {
            let mut w = world().await;
            let relay_pool = sqlx::postgres::PgPoolOptions::new()
                .max_connections(4)
                .connect_with(w.pool().connect_options().as_ref().clone())
                .await
                .unwrap();
            w.module = w.module.clone().with_inbox_database(
                driver::database::PostgresDatabase::from_pool(relay_pool.clone()),
            );
            let peer = RemotePeer::new();
            let mut templates = Vec::new();
            let raw=serde_json::to_vec(&json!({"type":"Create","actor":peer.actor_url,"to":"Public","object":{"content":"x".repeat(48*1024)}})).unwrap();
            for _ in 0..profiles {
                let p = w.actor_profile().await;
                if linked {
                    link(&w, &p).await;
                }
                let (parts, _) = request(&w, &p, &peer, &raw, "application/ld+json; charset=utf-8")
                    .await
                    .into_parts();
                templates.push(parts);
            }
            capacity(&w).await;
            metrics.0.lock().unwrap().clear();
            let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
            let active = running.clone();
            let pool = w.pool().clone();
            let rp = relay_pool.clone();
            let sampler = tokio::spawn(async move {
                let (mut samples, mut lock_samples, mut locks_max, mut pool_max, mut rss_max) =
                    (0, 0, 0, 0, 0);
                while active.load(std::sync::atomic::Ordering::Relaxed) {
                    let locks:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock'").fetch_one(&pool).await.unwrap();
                    samples += 1;
                    if locks > 0 {
                        lock_samples += 1;
                    }
                    locks_max = locks_max.max(locks);
                    pool_max = pool_max.max((rp.size() as usize).saturating_sub(rp.num_idle()));
                    rss_max = rss_max.max(rss_kib());
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                json!({"samples":samples,"lock_samples":lock_samples,"max_lock_waiters":locks_max,"max_pool_in_use":pool_max,"rss_peak_kib":rss_max})
            });
            let cpu = cpu_ticks();
            let began = std::time::Instant::now();
            let mut latency = Vec::new();
            let mut statuses = HashMap::new();
            let mut tasks = tokio::task::JoinSet::new();
            let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
            for first in (0..total).step_by(burst) {
                tokio::time::sleep_until(tokio::time::Instant::from_std(
                    began + std::time::Duration::from_millis(first as u64 * 10),
                ))
                .await;
                for i in first..(first + burst).min(total) {
                    let template = &templates[i % profiles];
                    let mut req = Request::new(Body::from(raw.clone()));
                    *req.method_mut() = template.method.clone();
                    *req.uri_mut() = template.uri.clone();
                    *req.headers_mut() = template.headers.clone();
                    let router = w.inbox_router();
                    let permit = semaphore.clone().acquire_owned().await.unwrap();
                    tasks.spawn(async move {
                        let _permit = permit;
                        let at = std::time::Instant::now();
                        let r = router.oneshot(req).await.unwrap();
                        (r.status().as_u16(), at.elapsed().as_micros() as u64)
                    });
                }
            }
            while let Some(r) = tasks.join_next().await {
                let (status, us) = r.unwrap();
                *statuses.entry(status).or_insert(0usize) += 1;
                latency.push(us);
            }
            let elapsed = began.elapsed().as_secs_f64();
            let ticks = cpu_ticks() - cpu;
            running.store(false, std::sync::atomic::Ordering::Relaxed);
            let sampled = sampler.await.unwrap();
            let times: serde_json::Map<String, Value> = metrics
                .0
                .lock()
                .unwrap()
                .iter_mut()
                .map(|(n, v)| (n.clone(), percentiles(v)))
                .collect();
            let jobs = count(&w, "post_relay_jobs").await;
            let buckets = count(&w, "post_relay_unlinked").await;
            println!(
                "I1_INTAKE {}",
                json!({"linked":linked,"profiles":profiles,"requests":total,"concurrency_bound":16,"target_rps":100,"raw_bytes":raw.len(),"seconds":elapsed,"rps":total as f64/elapsed,"status":statuses,"cpu_ticks":ticks,"latency_us":percentiles(&mut latency),"stages_us":times,"db":sampled,"jobs":jobs,"hour_buckets":buckets})
            );
            assert_eq!(statuses.get(&202), Some(&total));
            assert_eq!(jobs, if linked { total as i64 } else { 0 });
            assert_eq!(buckets, if linked { 0 } else { profiles as i64 });
            let m = w.module.clone();
            let consistent = w
                .module
                .transaction_manager()
                .transaction(move |c| {
                    Box::pin(async move { m.post_relay_store().check_reservations(c).await })
                })
                .await
                .unwrap();
            assert!(consistent);
            relay_pool.close().await;
        }
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn additive_migration_backfills_real_relations_but_never_undo_guards() {
    let w = world().await;
    let p = w.actor_profile().await;
    let peer = RemotePeer::new();
    let f = peer.activity_id("actual-follow");
    assert_eq!(
        post(&w, &p, &peer, &peer.follow(&f, &p.actor_url))
            .await
            .status(),
        StatusCode::ACCEPTED
    );
    let late = peer.activity_id("late-guard");
    assert_eq!(
        post(
            &w,
            &p,
            &peer,
            &peer.undo(&peer.activity_id("prior"), json!(late))
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    // Reproduce a populated Q1 schema. Only the new additive migration is reapplied.
    sqlx::raw_sql("ALTER TABLE profile_inbound_activities DROP COLUMN observed_relation; DELETE FROM _sqlx_migrations WHERE version=20261122000500;").execute(w.pool()).await.unwrap();
    sqlx::migrate!("../migrations").run(w.pool()).await.unwrap();
    let rows: Vec<(String, bool)> =
        sqlx::query_as("SELECT activity_type,observed_relation FROM profile_inbound_activities")
            .fetch_all(w.pool())
            .await
            .unwrap();
    assert!(rows.contains(&("Follow".into(), true)));
    assert!(rows.contains(&("UndoFollow".into(), false)));
    assert!(rows.contains(&("Undo".into(), false)));
}
#[test_with::env(DATABASE_URL)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_hour_bucket_and_last_profile_quota_slot_are_atomic() {
    let w = world().await;
    let p = w.actor_profile().await;
    let peer = RemotePeer::new();
    let raw = serde_json::to_vec(&json!({"type":"Create","actor":peer.actor_url})).unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..16 {
        let req = request(&w, &p, &peer, &raw, "application/activity+json").await;
        let router = w.inbox_router();
        tasks.spawn(async move { router.oneshot(req).await.unwrap().status() });
    }
    while let Some(r) = tasks.join_next().await {
        assert_eq!(r.unwrap(), StatusCode::ACCEPTED);
    }
    assert_eq!(count(&w, "post_relay_unlinked").await, 1);
    let n: i64 = sqlx::query_scalar("SELECT count FROM post_relay_unlinked")
        .fetch_one(w.pool())
        .await
        .unwrap();
    assert_eq!(n, 16);
    link(&w, &p).await;
    assert_eq!(
        post(
            &w,
            &p,
            &peer,
            &json!({"type":"Create","actor":peer.actor_url})
        )
        .await
        .status(),
        StatusCode::ACCEPTED
    );
    sqlx::query("UPDATE post_relay_profile_quota SET body_count=1999 WHERE profile_id=$1")
        .bind(p.id.as_ref())
        .execute(w.pool())
        .await
        .unwrap();
    for _ in 0..8 {
        let req = request(&w, &p, &peer, &raw, "application/activity+json").await;
        let router = w.inbox_router();
        tasks.spawn(async move { router.oneshot(req).await.unwrap().status() });
    }
    let (mut accepted, mut limited) = (0, 0);
    while let Some(r) = tasks.join_next().await {
        match r.unwrap() {
            StatusCode::ACCEPTED => accepted += 1,
            StatusCode::TOO_MANY_REQUESTS => limited += 1,
            other => panic!("unexpected {other}"),
        }
    }
    assert_eq!((accepted, limited), (1, 7));
    assert_eq!(count(&w, "post_relay_jobs").await, 2);
    sqlx::query("UPDATE post_relay_profile_quota SET body_count=2 WHERE profile_id=$1")
        .bind(p.id.as_ref())
        .execute(w.pool())
        .await
        .unwrap();
    let m = w.module.clone();
    assert!(w
        .module
        .transaction_manager()
        .transaction(move |c| Box::pin(
            async move { m.post_relay_store().check_reservations(c).await }
        ))
        .await
        .unwrap());
}
