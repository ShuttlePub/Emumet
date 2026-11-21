//! S1 orchestration tests: real DB, real HTTP, counted KDF and deterministic stop points.
use super::*;
mod measurement;
use crate::service::post_relay::delivery::{DeliveryPause, RelayDelivery};
use base64::Engine;
use kernel::interfaces::{
    database::{Transaction, TransactionalDatabaseConnection},
    event::EventApplier,
    home_link::{DependOnHomeLinkStore, HomeLinkStore},
    permission::{DependOnPermissionWriter, PermissionWriter, RelationTarget},
    post_relay::*,
    read_model::{
        AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
    },
    repository::{AggregateRepository, DependOnAccountRepository, DependOnProfileRepository},
};
use kernel::prelude::entity::Account;
use kernel::prelude::entity::{
    AccountId, AccountIsBot, AccountName, AuthAccountId, Nanoid, Profile, ProfileId,
};
use std::time::{Duration, Instant};
use tokio::sync::Notify;
use wiremock::{Mock, MockServer, ResponseTemplate};
#[derive(Clone, Default)]
pub(super) struct TestTransport {
    production: Option<driver::post_relay_delivery::HttpPostRelayTransport>,
    peer: Option<String>,
    entered: Arc<Notify>,
    release: Option<Arc<Notify>>,
    calls: Arc<AtomicUsize>,
    late: bool,
}
impl PostRelayTransport for TestTransport {
    fn origin_allowed(&self, origin: &str) -> bool {
        self.production
            .as_ref()
            .is_none_or(|p| p.origin_allowed(origin))
    }
    async fn send(&self, r: &RelayHttpRequest<'_>) -> RelayHttpResult {
        if let Some(p) = &self.production {
            return p.send(r).await;
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        let Some(peer) = &self.peer else {
            return RelayHttpResult::Transient;
        };
        let mut request = reqwest::Client::new()
            .post(format!("{peer}/internal/v1/emumet/activities"))
            .body(r.envelope.to_vec());
        for (k, v) in r.headers {
            request = request.header(k, v);
        }
        let work = async {
            let response = request.send().await.unwrap();
            self.entered.notify_one();
            if let Some(release) = &self.release {
                release.notified().await;
            }
            RelayHttpResult::Response {
                status: response.status().as_u16(),
                retry_after_secs: None,
            }
        };
        if self.late {
            let result = work.await;
            tokio::time::sleep_until(
                tokio::time::Instant::from_std(r.deadline) + Duration::from_millis(20),
            )
            .await;
            result
        } else {
            tokio::time::timeout_at(tokio::time::Instant::from_std(r.deadline), work)
                .await
                .unwrap_or(RelayHttpResult::Transient)
        }
    }
}
impl DependOnPostRelayTransport for TestModule {
    type PostRelayTransport = TestTransport;
    fn post_relay_transport(&self) -> &TestTransport {
        &self.transport
    }
}
pub(super) struct NoopPermission;
impl PermissionWriter for NoopPermission {
    async fn create_relation(
        &self,
        _: &RelationTarget,
        _: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
    async fn delete_relation(
        &self,
        _: &RelationTarget,
        _: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        Ok(())
    }
}
impl DependOnPermissionWriter for TestModule {
    type PermissionWriter = NoopPermission;
    fn permission_writer(&self) -> &NoopPermission {
        &NoopPermission
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

async fn admit_job(m: &TestModule, p: &Profile, receipt: &str, manual: bool) {
    admit_payload(
        m,
        p,
        receipt,
        manual,
        " { \"message\": \"こんにちは  世界\" }\n"
            .as_bytes()
            .to_vec(),
    )
    .await;
}
async fn admit_payload(
    m: &TestModule,
    p: &Profile,
    receipt: &str,
    manual: bool,
    envelope: Vec<u8>,
) {
    let db = &m.database;
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
                    receipt: receipt.into(),
                    received_at: now,
                    recipient,
                    envelope
                }
            )
            .await
            .unwrap(),
        Admission::Queued
    );
    t.commit().await.unwrap();
    if manual {
        let mut t = db.get_transaction().await.unwrap();
        let lease = db
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
                &lease,
                AttemptResult::Permanent { status: Some(403) },
                None,
                0,
            )
            .await
            .unwrap();
        t.commit().await.unwrap();
        let req = OperatorRequest {
            id: format!("op-{receipt}"),
            profile_id: p.id().clone(),
            receipt: Some(receipt.into()),
            action: OperatorAction::Redeliver,
            operator: "test-ops".into(),
            reason: "test".into(),
        };
        let mut t = db.get_transaction().await.unwrap();
        db.post_relay_store()
            .begin_operator(t.connection(), &req)
            .await
            .unwrap();
        db.post_relay_store()
            .operator_redeliver(t.connection(), &req.id)
            .await
            .unwrap();
        t.commit().await.unwrap();
    }
}
async fn revoke(m: &TestModule, a: &Account, p: &Profile, path: u8) {
    let db = &m.database;
    match path {
        0 => {
            let mut t = db.get_transaction().await.unwrap();
            db.home_link_store()
                .lock_accounts(t.connection(), std::slice::from_ref(a.id()))
                .await
                .unwrap();
            db.home_link_store()
                .lock_profiles(t.connection(), std::slice::from_ref(p.id()))
                .await
                .unwrap();
            let links = db
                .home_link_store()
                .list(t.connection(), p.id())
                .await
                .unwrap();
            db.home_link_store()
                .unlink(t.connection(), p.id(), &links[0].id)
                .await
                .unwrap();
            t.commit().await.unwrap();
        }
        1 => {
            let mut c = db.connection().await.unwrap();
            let current = db.account_repository().load(&mut c, a.id()).await.unwrap();
            db.account_repository()
                .save(
                    &mut c,
                    Account::deactivate(a.id().clone(), current.version().clone()),
                )
                .await
                .unwrap();
        }
        _ => {
            // Simulate an old producer that persisted an event before L1's atomic hook existed.
            let mut c = db.connection().await.unwrap();
            let current = db.account_repository().load(&mut c, a.id()).await.unwrap();
            sqlx::query("INSERT INTO account_events(version,id,event_name,data) VALUES($1,$2,'account_deactivated',$3)").bind(*current.version().as_ref()+1).bind(a.id().as_ref()).bind(serde_json::json!({"type":"Deactivated"})).execute(&mut *c).await.unwrap();
            drop(c);
            crate::projection::ProjectAccountBatch::project_batch(m)
                .await
                .unwrap();
        }
    }
}
async fn setup() -> (
    Env,
    TestModule,
    Arc<AtomicUsize>,
    MockServer,
    Account,
    Profile,
) {
    let env = Env::new().await;
    let (mut m, kdf) = env.process();
    m.ensure_instance_identity().await.unwrap();
    let peer = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&peer)
        .await;
    m.transport.peer = Some(peer.uri());
    let (a, mut ps) = fixture(&m.database, 1, true).await;
    (env, m, kdf, peer, a, ps.remove(0))
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[test_with::env(DATABASE_URL)]
async fn s1_p0_p1_p2_p4_three_revokers_auto_and_manual_no_body_resurrection() {
    for manual in [false, true] {
        for path in 0..3 {
            for point in [0u8, 1, 2, 4] {
                let (env, m, _, _peer, a, p) = setup().await;
                admit_job(&m, &p, "race", manual).await;
                let pause = Arc::new(DeliveryPause {
                    point,
                    ..Default::default()
                });
                let worker =
                    Arc::new(RelayDelivery::new(Arc::new(m.clone())).with_pause(pause.clone()));
                let lease = worker.claim(1).await.unwrap().remove(0);
                let w = worker.clone();
                let send = tokio::spawn(async move { w.deliver(lease).await });
                if tokio::time::timeout(Duration::from_secs(10), pause.reached.notified())
                    .await
                    .is_err()
                {
                    panic!(
                        "checkpoint {point} manual={manual} path={path} not reached: {:?}",
                        send.await
                    );
                }
                let start = Instant::now();
                if point == 1 {
                    // Permission holds the common gate until commit: unlink cannot pass it.
                    let revoker = tokio::spawn({
                        let (m, a, p) = (m.clone(), a.clone(), p.clone());
                        async move { revoke(&m, &a, &p, path).await }
                    });
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    assert!(!revoker.is_finished());
                    pause.resume.notify_one();
                    tokio::time::timeout(Duration::from_secs(3), revoker)
                        .await
                        .unwrap()
                        .unwrap();
                } else {
                    tokio::time::timeout(Duration::from_secs(3), revoke(&m, &a, &p, path))
                        .await
                        .unwrap();
                }
                eprintln!(
                    "S1_REVOKE path={path} point={point} manual={manual} ms={:.3}",
                    start.elapsed().as_secs_f64() * 1000.0
                );
                pause.resume.notify_one();
                send.await.unwrap().unwrap();
                worker.maintain().await.unwrap();
                let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
                    .fetch_one(&env.iso.pool)
                    .await
                    .unwrap();
                assert_eq!(rows, 0);
                assert_eq!(
                    m.transport.calls.load(Ordering::SeqCst),
                    usize::from(point != 0)
                );
            }
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[test_with::env(DATABASE_URL)]
async fn s1_p3_silent_http_has_zero_db_holds_and_all_revokers_finish() {
    for manual in [false, true] {
        for path in 0..3 {
            let (env, m, _, peer, a, p) = setup().await;
            admit_job(&m, &p, "silent", manual).await;
            peer.reset().await;
            let entered = m.transport.entered.clone();
            Mock::given(wiremock::matchers::method("POST"))
                .respond_with(move |_: &wiremock::Request| {
                    entered.notify_one();
                    ResponseTemplate::new(204).set_delay(Duration::from_secs(30))
                })
                .mount(&peer)
                .await;
            let worker = Arc::new(RelayDelivery::new(Arc::new(m.clone())));
            let lease = worker.claim(1).await.unwrap().remove(0);
            let w = worker.clone();
            let send = tokio::spawn(async move { w.deliver(lease).await });
            m.transport.entered.notified().await;
            let locks:i64=sqlx::query_scalar("SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND state='idle in transaction'").fetch_one(&env.iso.pool).await.unwrap();
            assert_eq!(locks, 0);
            let locks:i64=sqlx::query_scalar("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND pid IN(SELECT pid FROM pg_stat_activity WHERE datname=current_database())").fetch_one(&env.iso.pool).await.unwrap();
            assert_eq!(locks, 0);
            tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    if env.iso.pool.num_idle() == env.iso.pool.size() as usize {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .expect("HTTP retained a checked-out DB connection");
            let revoked = Instant::now();
            tokio::time::timeout(Duration::from_secs(3), revoke(&m, &a, &p, path))
                .await
                .unwrap();
            eprintln!(
                "S1_SILENT_REVOKE path={path} manual={manual} ms={:.3}",
                revoked.elapsed().as_secs_f64() * 1000.0
            );
            assert!(!send.is_finished());
            send.abort();
            let _ = send.await;
            worker.maintain().await.unwrap();
            let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
                .fetch_one(&env.iso.pool)
                .await
                .unwrap();
            assert_eq!(rows, 0);
        }
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[test_with::env(DATABASE_URL)]
async fn s1_p1_timeout_rollback_does_not_send_and_p2_expired_does_not_send() {
    for point in [1u8, 2] {
        let (env, m, _, _peer, a, p) = setup().await;
        admit_job(&m, &p, "expiry", false).await;
        let pause = Arc::new(DeliveryPause {
            point,
            ..Default::default()
        });
        let w = Arc::new(RelayDelivery::new(Arc::new(m.clone())).with_pause(pause.clone()));
        let lease = w.claim(1).await.unwrap().remove(0);
        let worker = w.clone();
        let send = tokio::spawn(async move { worker.deliver(lease).await });
        pause.reached.notified().await;
        if point == 1 {
            tokio::time::sleep(Duration::from_secs(6)).await;
            assert!(send.await.unwrap().is_err());
            revoke(&m, &a, &p, 0).await;
        } else {
            tokio::time::sleep(Duration::from_secs(11)).await;
            pause.resume.notify_one();
            send.await.unwrap().unwrap();
        }
        assert_eq!(m.transport.calls.load(Ordering::SeqCst), 0);
        let _ = env;
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[test_with::env(DATABASE_URL)]
async fn s1_in_time_signature_immutable_bytes_cached_kdf_and_late_result_is_not_success() {
    let (env, mut m, kdf, peer, _, p) = setup().await;
    let worker = RelayDelivery::new(Arc::new(m.clone()));
    for i in 0..4 {
        admit_job(&m, &p, &format!("sig-{i}"), false).await;
        let lease = worker.claim(1).await.unwrap().remove(0);
        worker.deliver(lease).await.unwrap();
    }
    assert_eq!(kdf.load(Ordering::SeqCst), 1);
    assert_eq!(m.signer_factory.prepares.load(Ordering::SeqCst), 1);
    let requests = peer.received_requests().await.unwrap();
    let (_, public, _, _) = env.stored_key().await;
    for request in &requests {
        assert_eq!(
            request.body,
            " { \"message\": \"こんにちは  世界\" }\n".as_bytes()
        );
        let value = request.headers.get("signature").unwrap().to_str().unwrap();
        let signature = value
            .split("signature=\"")
            .nth(1)
            .unwrap()
            .trim_end_matches('"');
        let signature = base64::engine::general_purpose::STANDARD
            .decode(signature)
            .unwrap();
        let input = format!(
            "(request-target): post /internal/v1/emumet/activities\nhost: {}\ndate: {}\ndigest: {}",
            request.headers["host"].to_str().unwrap(),
            request.headers["date"].to_str().unwrap(),
            request.headers["digest"].to_str().unwrap()
        );
        assert!(Rsa2048Verifier
            .verify(input.as_bytes(), &signature, public.as_bytes())
            .unwrap());
    }
    m.transport.late = true;
    let worker = RelayDelivery::new(Arc::new(m.clone()));
    admit_job(&m, &p, "late", false).await;
    let lease = worker.claim(1).await.unwrap().remove(0);
    worker.deliver(lease).await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_jobs WHERE receipt='late'")
        .fetch_one(&env.iso.pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn s1_outgoing_headers_match_canonical_cavage_vectors() {
    let vectors: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../docs/contracts/post-relay-v1-fixtures/signature-vectors.json"
    ))
    .unwrap();
    for v in vectors["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["expect"] == "valid")
    {
        let signature_header = v["signature_header"].as_str().unwrap();
        let signature = base64::engine::general_purpose::STANDARD
            .decode(
                signature_header
                    .split("signature=\"")
                    .nth(1)
                    .unwrap()
                    .trim_end_matches('"'),
            )
            .unwrap();
        let headers = crate::service::post_relay::delivery::signed_headers_at(
            &format!("https://{}", v["host"].as_str().unwrap()),
            v["body"].as_str().unwrap().as_bytes(),
            v["key_id"].as_str().unwrap(),
            v["date"].as_str().unwrap(),
            |input| {
                assert_eq!(input, v["signing_string"].as_str().unwrap().as_bytes());
                Ok(signature.clone())
            },
        )
        .unwrap();
        assert_eq!(
            headers.iter().find(|(k, _)| k == "digest").unwrap().1,
            v["digest_header"].as_str().unwrap()
        );
        assert_eq!(
            headers.iter().find(|(k, _)| k == "signature").unwrap().1,
            signature_header
        );
        assert!(Rsa2048Verifier.verify(v["signing_string"].as_str().unwrap().as_bytes(),&signature,include_bytes!("../../../../docs/contracts/post-relay-v1-fixtures/keys/emumet-a-instance-active.pub.pem")).unwrap());
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[test_with::env(DATABASE_URL)]
async fn s1_p4_observed_in_time_remains_valid_after_deadline_before_db_save() {
    let (env, m, _, _peer, _, p) = setup().await;
    admit_job(&m, &p, "late-save", false).await;
    let pause = Arc::new(DeliveryPause {
        point: 4,
        ..Default::default()
    });
    let w = Arc::new(RelayDelivery::new(Arc::new(m)).with_pause(pause.clone()));
    let lease = w.claim(1).await.unwrap().remove(0);
    let worker = w.clone();
    let send = tokio::spawn(async move { worker.deliver(lease).await });
    tokio::time::timeout(Duration::from_secs(5), pause.reached.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(11)).await;
    pause.resume.notify_one();
    assert_eq!(send.await.unwrap().unwrap(), Recorded::Applied);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
        .fetch_one(&env.iso.pool)
        .await
        .unwrap();
    assert_eq!(rows, 0);
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[test_with::env(DATABASE_URL)]
async fn s1_401_retry_changes_only_outer_date_signature_and_not_envelope() {
    let (env, m, kdf, peer, _, p) = setup().await;
    peer.reset().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&peer)
        .await;
    admit_job(&m, &p, "retry", false).await;
    let w = RelayDelivery::new(Arc::new(m));
    let lease = w.claim(1).await.unwrap().remove(0);
    w.deliver(lease).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    sqlx::query("UPDATE post_relay_jobs SET due_at=clock_timestamp() WHERE receipt='retry'")
        .execute(&env.iso.pool)
        .await
        .unwrap();
    let lease = w.claim(1).await.unwrap().remove(0);
    w.deliver(lease).await.unwrap();
    let requests = peer.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body, requests[1].body);
    assert_eq!(requests[0].headers["digest"], requests[1].headers["digest"]);
    assert_ne!(requests[0].headers["date"], requests[1].headers["date"]);
    assert_ne!(
        requests[0].headers["signature"],
        requests[1].headers["signature"]
    );
    assert_eq!(kdf.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[test_with::env(DATABASE_URL)]
async fn s1_unlink_gate_and_precommit_checkpoints_three_revokers_auto_manual() {
    for checkpoint in ["BEFORE", "AFTER"] {
        for manual in [false, true] {
            for path in 0..3 {
                let (env, m, _, _peer, a, p) = setup().await;
                let _ = m.unlock_instance_signing_key().await.unwrap();
                admit_job(&m, &p, "unlink-first", manual).await;
                let worker = Arc::new(RelayDelivery::new(Arc::new(m.clone())));
                let lease = worker.claim(1).await.unwrap().remove(0);
                sqlx::query("CREATE FUNCTION s1_pause_unlink() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(98765); RETURN NEW; END $$").execute(&env.iso.pool).await.unwrap();
                sqlx::query(&format!("CREATE TRIGGER s1_unlink_checkpoint {checkpoint} UPDATE OF state ON profile_links FOR EACH ROW WHEN(OLD.state<>'unlinked' AND NEW.state='unlinked') EXECUTE FUNCTION s1_pause_unlink()")).execute(&env.iso.pool).await.unwrap();
                let mut blocker = env.iso.pool.begin().await.unwrap();
                sqlx::query("SELECT pg_advisory_xact_lock(98765)")
                    .execute(&mut *blocker)
                    .await
                    .unwrap();
                let revoker = tokio::spawn({
                    let (m, a, p) = (m.clone(), a.clone(), p.clone());
                    async move { revoke(&m, &a, &p, path).await }
                });
                tokio::time::timeout(Duration::from_secs(2),async{loop{let paused:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_locks WHERE locktype='advisory' AND NOT granted AND objid=98765)").fetch_one(&env.iso.pool).await.unwrap();if paused{break}tokio::time::sleep(Duration::from_millis(5)).await;}}).await.unwrap();
                let w = worker.clone();
                let send = tokio::spawn(async move { w.deliver(lease).await });
                tokio::time::sleep(Duration::from_millis(50)).await;
                assert_eq!(m.transport.calls.load(Ordering::SeqCst), 0);
                assert!(!send.is_finished());
                blocker.rollback().await.unwrap();
                revoker.await.unwrap();
                send.await.unwrap().unwrap();
                worker.maintain().await.unwrap();
                assert_eq!(m.transport.calls.load(Ordering::SeqCst), 0);
                let bodies: i64 = sqlx::query_scalar("SELECT count(*) FROM post_relay_bodies")
                    .fetch_one(&env.iso.pool)
                    .await
                    .unwrap();
                assert_eq!(bodies, 0);
            }
        }
    }
}
