//! Webhook 配送 worker の停止 (協調停止・graceful shutdown・強制中断) と再起動で、通知が消失・
//! 重複しないことの実 DB テスト。受信側 (wiremock) が見た event ID を数えて固定する。
//!
//! at-least-once のため、送信後・記録前にプロセスが死んだ場合の再送は仕様どおり (lease 失効後に同じ
//! event ID)。それ以外 (協調停止・graceful shutdown・再起動の繰り返し) では同じイベントを 2 回送らない。

use super::test_support::{observation, GateTestApp, DOWN, GATE_ROUTE_TEST_LOCK, HEALTHY};
use super::webhook_tests::{call, key, loopback_sender, ENDPOINTS, GATE, SECRET};
use crate::webhook_worker;
use application::service::registration_gate_webhook::DeliverRegistrationGateWebhooksUseCase;
use axum::http::{Method, StatusCode};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::WebhookCycleReport;
use kernel::prelude::entity::WebhookDeliveryPolicy;
use sqlx::PgConnection;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct Fixture {
    app: GateTestApp,
    receiver: MockServer,
    _keto: MockServer,
}

impl Fixture {
    /// 全カテゴリを購読する送信先を 1 つ登録した状態 (イベントは登録後に発生させる)。
    async fn new(receiver: MockServer) -> Self {
        let keto = MockServer::start().await;
        let app = GateTestApp::with_webhook_wiring(&keto, loopback_sender(), key()).await;
        let (status, _, text) = call(
            &app,
            Some(&app.admin_a),
            Method::POST,
            ENDPOINTS,
            Some(serde_json::json!({
                "url": format!("{}/hook", receiver.uri()),
                "secret": SECRET,
                "categories": ["outage", "recovery", "auto_stop", "override"],
            })),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{text}");
        Self {
            app,
            receiver,
            _keto: keto,
        }
    }

    async fn event_count(&self) -> i64 {
        gate_event_count(&self.app).await
    }

    /// (event_id, status, attempt_count, lease_owner)
    async fn rows(&self) -> Vec<(i64, String, i32, Option<String>)> {
        let mut db = self
            .app
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let con: &mut PgConnection = &mut db;
        sqlx::query_as(
            "SELECT event_id, status, attempt_count, lease_owner \
             FROM registration_gate_webhook_deliveries ORDER BY event_id",
        )
        .fetch_all(con)
        .await
        .unwrap()
    }

    async fn generate_events(&self, at_least: i64) -> i64 {
        generate_gate_events(&self.app, at_least).await
    }

    async fn gate(&self) -> serde_json::Value {
        call(&self.app, Some(&self.app.admin_a), Method::GET, GATE, None)
            .await
            .1
    }

    async fn cycle(&self, worker: &str, stop: &(dyn Fn() -> bool + Sync)) -> WebhookCycleReport {
        self.cycle_with(worker, &WebhookDeliveryPolicy::default(), stop)
            .await
    }

    async fn cycle_with(
        &self,
        worker: &str,
        policy: &WebhookDeliveryPolicy,
        stop: &(dyn Fn() -> bool + Sync),
    ) -> WebhookCycleReport {
        let jitter = || 1.0;
        self.app
            .module
            .deliver_registration_gate_webhooks(worker, policy, &jitter, stop)
            .await
            .expect("delivery cycle")
    }

    /// 受信側が受け取った (event ID, 配送試行番号)。
    async fn received(&self) -> Vec<(String, String)> {
        received(&self.receiver).await
    }

    async fn wait_for_requests(&self, at_least: usize) {
        for _ in 0..500 {
            if self.received().await.len() >= at_least {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the receiver never saw {at_least} request(s)");
    }

    async fn wait_until_all_delivered(&self, total: usize) {
        for _ in 0..1500 {
            let rows = self.rows().await;
            if rows.len() == total && rows.iter().all(|row| row.1 == "delivered") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("not every delivery was delivered: {:?}", self.rows().await);
    }

    fn app_arc(&self) -> Arc<crate::handler::AppModule> {
        Arc::new(self.app.module.clone())
    }
}

pub(super) async fn gate_event_count(app: &GateTestApp) -> i64 {
    let mut db = app.module.database_connection().connection().await.unwrap();
    let con: &mut PgConnection = &mut db;
    sqlx::query_scalar("SELECT count(*) FROM registration_gate_events")
        .fetch_one(con)
        .await
        .unwrap()
}

/// ゲートの open / DOWN / 回復を繰り返して、通知イベントを少なくとも `at_least` 件発生させる。
pub(super) async fn generate_gate_events(app: &GateTestApp, at_least: i64) -> i64 {
    let before = gate_event_count(app).await;
    for _ in 0..8 {
        app.observe(observation(HEALTHY, HEALTHY)).await;
        let (_, gate, _) = call(app, Some(&app.admin_a), Method::GET, GATE, None).await;
        if gate["accepting"] == false {
            let (status, _, text) = call(
                app,
                Some(&app.admin_a),
                Method::POST,
                &format!("{GATE}/open"),
                Some(serde_json::json!({"revision": gate["revision"]})),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{text}");
        }
        app.observe(observation(DOWN, DOWN)).await;
        app.observe(observation(HEALTHY, HEALTHY)).await;
        if gate_event_count(app).await - before >= at_least {
            break;
        }
    }
    let added = gate_event_count(app).await - before;
    assert!(added >= at_least, "only {added} gate events were produced");
    added
}

async fn received(receiver: &MockServer) -> Vec<(String, String)> {
    let header = |request: &wiremock::Request, name: &str| {
        request
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    receiver
        .received_requests()
        .await
        .expect("request recording")
        .iter()
        .map(|request| {
            (
                header(request, "x-emumet-event-id"),
                header(request, "x-emumet-delivery-attempt"),
            )
        })
        .collect()
}

fn counts(received: &[(String, String)]) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for (event_id, _) in received {
        *counts.entry(event_id.clone()).or_insert(0) += 1;
    }
    counts
}

async fn mount_fast(receiver: &MockServer) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(204))
        .mount(receiver)
        .await;
}

async fn mount_slow(receiver: &MockServer, delay: Duration) {
    Mock::given(any())
        .respond_with(ResponseTemplate::new(204).set_delay(delay))
        .mount(receiver)
        .await;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_cooperative_stop_mid_cycle_keeps_the_rest_due_and_a_restart_sends_each_event_once()
{
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let receiver = MockServer::start().await;
    mount_fast(&receiver).await;
    let fx = Fixture::new(receiver).await;
    let total = fx.generate_events(3).await;
    let gate_before = fx.gate().await;

    // 1 件送ったところで停止要求が立つ。
    let polls = AtomicUsize::new(0);
    let stop = || polls.fetch_add(1, Ordering::SeqCst) >= 1;
    let report = fx.cycle("worker-before-stop", &stop).await;
    assert_eq!((report.attempted, report.delivered), (1, 1));

    let rows = fx.rows().await;
    assert_eq!(rows.len() as i64, total, "every event has a delivery row");
    assert_eq!(rows.iter().filter(|row| row.1 == "delivered").count(), 1);
    for row in rows.iter().filter(|row| row.1 != "delivered") {
        assert_eq!(
            (row.1.as_str(), row.2, row.3.as_deref()),
            ("pending", 0, None),
            "an unsent event stays due: not failed, no attempt counted, no lease held"
        );
    }
    assert_eq!(fx.received().await.len(), 1);

    // 再起動 (別 worker) が残りを配送する。受信側から見て各イベントは 1 回。
    let report = fx.cycle("worker-after-restart", &|| false).await;
    assert_eq!(report.delivered as i64, total - 1);
    // 以後の再起動では何も送らず、行も増えない。
    for round in 0..3 {
        let report = fx
            .cycle(&format!("worker-restart-{round}"), &|| false)
            .await;
        assert_eq!(report.attempted, 0);
    }
    let received = fx.received().await;
    let counts = counts(&received);
    assert_eq!(counts.len() as i64, total, "no event was lost");
    assert!(
        counts.values().all(|count| *count == 1),
        "no event was sent twice: {counts:?}"
    );
    assert_eq!(fx.rows().await.len() as i64, total);
    assert_eq!(fx.event_count().await, total, "stopping creates no event");
    assert_eq!(
        fx.gate().await,
        gate_before,
        "stopping never moves the gate"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_worker_stop_waits_for_the_in_flight_send_and_a_restart_does_not_resend_it() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let receiver = MockServer::start().await;
    mount_slow(&receiver, Duration::from_millis(800)).await;
    let fx = Fixture::new(receiver).await;
    let total = fx.generate_events(3).await as usize;

    let worker = webhook_worker::spawn(fx.app_arc());
    fx.wait_for_requests(1).await;
    // 送信中に停止する。worker は in-flight の結果記録まで済ませてから戻る。
    assert!(
        worker.stop_and_wait(Duration::from_secs(10)).await,
        "the worker stops within the grace period"
    );
    let rows = fx.rows().await;
    assert_eq!(
        rows.iter().filter(|row| row.1 == "delivered").count(),
        1,
        "the in-flight send was recorded: {rows:?}"
    );
    assert!(
        rows.iter()
            .all(|row| row.1 != "in_flight" && row.3.is_none()),
        "no lease is left behind: {rows:?}"
    );
    assert_eq!(fx.received().await.len(), 1);

    // 再起動後、残りだけが送られる。最初の 1 件は再送されない。
    let worker = webhook_worker::spawn(fx.app_arc());
    fx.wait_until_all_delivered(total).await;
    assert!(worker.stop_and_wait(Duration::from_secs(10)).await);
    let counts = counts(&fx.received().await);
    assert_eq!(counts.len(), total, "no event was lost");
    assert!(
        counts.values().all(|count| *count == 1),
        "no event was sent twice: {counts:?}"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_abort_mid_send_is_resent_once_after_the_lease_expires_and_nothing_else_repeats() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let receiver = MockServer::start().await;
    // 最初の 1 リクエストだけ長く待たせる (その間に worker を殺す)。以降は即応答。
    Mock::given(any())
        .respond_with(ResponseTemplate::new(204).set_delay(Duration::from_secs(5)))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&receiver)
        .await;
    mount_fast(&receiver).await;
    let fx = Fixture::new(receiver).await;
    let total = fx.generate_events(3).await;
    let gate_before = fx.gate().await;

    let dead = {
        let module = fx.app_arc();
        tokio::spawn(async move {
            let policy = WebhookDeliveryPolicy {
                lease_duration: time::Duration::seconds(2),
                ..WebhookDeliveryPolicy::default()
            };
            let jitter = || 1.0;
            let stop = || false;
            module
                .deliver_registration_gate_webhooks("worker-killed", &policy, &jitter, &stop)
                .await
        })
    };
    fx.wait_for_requests(1).await;
    // プロセス死の模擬: 送信中に future ごと中断する (結果は記録されない)。
    dead.abort();
    assert!(dead.await.unwrap_err().is_cancelled());
    let first = fx.received().await;
    let abandoned = first[0].0.clone();
    let abandoned_attempt: i64 = first[0].1.parse().unwrap();
    let rows = fx.rows().await;
    assert_eq!(
        rows.iter().filter(|row| row.1 == "in_flight").count(),
        1,
        "the aborted send still holds its lease: {rows:?}"
    );

    // lease が生きている間は別 worker も再送しない。他のイベントだけが送られる。
    let report = fx.cycle("worker-live", &|| false).await;
    assert_eq!(report.delivered as i64, total - 1);
    let counts_now = counts(&fx.received().await);
    assert_eq!(counts_now.len() as i64, total);
    assert!(
        counts_now.values().all(|count| *count == 1),
        "nothing is duplicated while the lease is alive: {counts_now:?}"
    );

    // lease の失効後、放置された 1 件だけが同じ event ID で 1 回再送される。
    tokio::time::sleep(Duration::from_millis(2400)).await;
    let report = fx.cycle("worker-live", &|| false).await;
    assert_eq!(report.delivered, 1, "only the abandoned event is resent");
    for round in 0..2 {
        let report = fx
            .cycle(&format!("worker-restart-{round}"), &|| false)
            .await;
        assert_eq!(report.attempted, 0);
    }
    let received = fx.received().await;
    let counts = counts(&received);
    assert_eq!(counts.len() as i64, total, "no event was lost");
    for (event_id, count) in &counts {
        assert_eq!(
            *count,
            if *event_id == abandoned { 2 } else { 1 },
            "{event_id}: only the abandoned event is sent again (at-least-once): {counts:?}"
        );
    }
    let resent_attempt: i64 = received
        .iter()
        .filter(|(event_id, _)| *event_id == abandoned)
        .map(|(_, attempt)| attempt.parse::<i64>().unwrap())
        .max()
        .unwrap();
    assert!(resent_attempt > abandoned_attempt, "{received:?}");
    assert!(fx.rows().await.iter().all(|row| row.1 == "delivered"));
    assert_eq!(fx.event_count().await, total, "no event was created");
    assert_eq!(fx.gate().await, gate_before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn webhook_events_created_while_the_worker_is_stopped_are_delivered_once_after_restart() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let receiver = MockServer::start().await;
    mount_fast(&receiver).await;
    let fx = Fixture::new(receiver).await;

    let first = fx.generate_events(2).await as usize;
    let worker = webhook_worker::spawn(fx.app_arc());
    fx.wait_until_all_delivered(first).await;
    assert!(worker.stop_and_wait(Duration::from_secs(10)).await);

    // worker が止まっている間に発生したイベント。
    let second = fx.generate_events(2).await as usize;
    let total = first + second;
    assert_eq!(
        fx.received().await.len(),
        first,
        "a stopped worker sends nothing"
    );

    // 再起動を繰り返しても、停止中のイベントは 1 回だけ配送され、行も通知も増えない。
    for _ in 0..3 {
        let worker = webhook_worker::spawn(fx.app_arc());
        fx.wait_until_all_delivered(total).await;
        assert!(worker.stop_and_wait(Duration::from_secs(10)).await);
    }
    let counts = counts(&fx.received().await);
    assert_eq!(counts.len(), total, "no event was lost");
    assert!(
        counts.values().all(|count| *count == 1),
        "no event was sent twice: {counts:?}"
    );
    assert_eq!(fx.rows().await.len(), total);
    assert_eq!(fx.event_count().await as usize, total);
}
