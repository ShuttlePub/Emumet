//! probe worker の停止 (shutdown) と再起動で、強制 probe (negative canary) とゲートの通知が
//! 消失・重複・捏造されず、fail-open もしないことの実 DB テスト。Kratos は axum の模擬サーバー。
//!
//! worker は停止要求で実行中のサイクルの future を drop する (= 途中中断)。ここでは、中断が
//! canary 準備前・送信中・クリーンアップ中のどこで起きても、再起動後のサイクルが収束し、中断だけで
//! 通知イベントが増えないことを固定する。

use super::{probe, worker};
use crate::handler::AppModule;
use crate::route::registration_gate::test_support::{GateTestApp, GATE_ROUTE_TEST_LOCK};
use application::service::registration_enforcement_probe::RegistrationEnforcementMonitorUseCase;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use driver::registration_enforcement_probe::{HttpKratosEnforcementProbe, ProbeTargetSettings};
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::repository::{
    DependOnRegistrationEnforcementProbeRepository, EnforcementProbeId,
    RegistrationEnforcementProbeRepository,
};
use sqlx::PgConnection;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{watch, Notify};
use wiremock::MockServer;

const INTERVAL: time::Duration = time::Duration::seconds(60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Stall {
    Initialize,
    Submit,
    Cleanup,
}

/// 模擬 Kratos。`stall` の地点に来たリクエストは応答せずに固まる (その間に worker を止める)。
struct Kratos {
    module: OnceLock<AppModule>,
    stall: Mutex<Option<Stall>>,
    entered: Notify,
    submitted_probe_ids: Mutex<Vec<String>>,
    cleaned_emails: Mutex<Vec<String>>,
}

impl Kratos {
    async fn maybe_stall(&self, at: Stall) {
        if *self.stall.lock().unwrap() == Some(at) {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
    }
}

async fn initialize(State(kratos): State<Arc<Kratos>>, Path(mode): Path<String>) -> Response {
    kratos.maybe_stall(Stall::Initialize).await;
    let body = serde_json::json!({
        "id": uuid::Uuid::new_v4().to_string(),
        "type": mode,
        "ui": {"nodes": [
            {"group": "default", "attributes": {"name": "csrf_token", "value": "csrf-value"}}
        ]},
    });
    let mut response = Json(body).into_response();
    if mode == "browser" {
        response.headers_mut().append(
            header::SET_COOKIE,
            "csrf_token_probe=cookie-value; Path=/".parse().unwrap(),
        );
    }
    response
}

async fn submit(
    State(kratos): State<Arc<Kratos>>,
    RawQuery(query): RawQuery,
    Json(body): Json<serde_json::Value>,
) -> Response {
    kratos.maybe_stall(Stall::Submit).await;
    let flow = query
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("flow=").map(str::to_string))
        .unwrap_or_default();
    let probe_id = body["transient_payload"]["registration_probe_id"]
        .as_str()
        .map(str::to_string);
    let email = body["traits"]["email"].as_str().unwrap_or_default();
    if let Some(id) = &probe_id {
        kratos.submitted_probe_ids.lock().unwrap().push(id.clone());
    }
    // 本物の hook と同じ: 予約済みの canary を受信済みとして記録し、登録は拒否する。
    let _ = kratos
        .module
        .get()
        .expect("module is set before serving")
        .receive_registration_enforcement_canary(probe_id.as_deref(), &flow, email)
        .await;
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"ui": {"messages": [{"id": 4_000_001}]}})),
    )
        .into_response()
}

async fn identities(State(kratos): State<Arc<Kratos>>, RawQuery(query): RawQuery) -> Response {
    kratos.maybe_stall(Stall::Cleanup).await;
    let email = query
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| {
            pair.strip_prefix("credentials_identifier=")
                .map(str::to_string)
        })
        .unwrap_or_default()
        .replace("%40", "@");
    kratos.cleaned_emails.lock().unwrap().push(email);
    Json(serde_json::json!([])).into_response()
}

struct Fixture {
    module: Arc<AppModule>,
    kratos: Arc<Kratos>,
    _keto: MockServer,
    _app: GateTestApp,
}

impl Fixture {
    async fn new() -> Self {
        let keto = MockServer::start().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/", listener.local_addr().unwrap());
        let app = GateTestApp::new(&keto).await;
        let probe = HttpKratosEnforcementProbe::new(vec![ProbeTargetSettings {
            public_url: base.clone(),
            admin_url: base,
        }]);
        let module = Arc::new(app.module.clone().with_enforcement_probe(probe));
        let kratos = Arc::new(Kratos {
            module: OnceLock::new(),
            stall: Mutex::new(None),
            entered: Notify::new(),
            submitted_probe_ids: Mutex::new(Vec::new()),
            cleaned_emails: Mutex::new(Vec::new()),
        });
        kratos.module.set((*module).clone()).ok();
        let router = Router::new()
            .route("/self-service/registration/{mode}", get(initialize))
            .route("/self-service/registration", post(submit))
            .route("/admin/identities", get(identities))
            .with_state(kratos.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            module,
            kratos,
            _keto: keto,
            _app: app,
        }
    }

    fn set_stall(&self, stall: Option<Stall>) {
        *self.kratos.stall.lock().unwrap() = stall;
    }

    async fn sql_scalar<T>(&self, sql: &str) -> T
    where
        T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
    {
        let mut db = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let con: &mut PgConnection = &mut db;
        sqlx::query_scalar(sql).fetch_one(con).await.unwrap()
    }

    async fn event_count(&self) -> i64 {
        self.sql_scalar("SELECT count(*) FROM registration_gate_events")
            .await
    }

    async fn make_due(&self) {
        let mut db = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let con: &mut PgConnection = &mut db;
        sqlx::query(
            "UPDATE registration_enforcement_probe_schedule \
             SET next_probe_at = now() - interval '1 second'",
        )
        .execute(con)
        .await
        .unwrap();
    }

    /// (probe_id, settled, received) を古い順に。
    async fn canaries(&self) -> Vec<(String, bool, bool)> {
        let mut db = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        let con: &mut PgConnection = &mut db;
        sqlx::query_as(
            "SELECT probe_id, settled, received_at IS NOT NULL \
             FROM registration_enforcement_canaries ORDER BY created_at, probe_id",
        )
        .fetch_all(con)
        .await
        .unwrap()
    }

    async fn evidence(&self) -> kernel::interfaces::repository::EnforcementProbeEvidence {
        let mut db = self
            .module
            .database_connection()
            .connection()
            .await
            .unwrap();
        self.module
            .registration_enforcement_probe_repository()
            .evidence(&mut db)
            .await
            .unwrap()
            .expect("evidence exists")
    }

    async fn enforcement_verified(&self) -> bool {
        self.module
            .refresh_registration_enforcement()
            .await
            .unwrap()
            .state
            .observation
            .enforcement_verified
    }

    /// 1 サイクル。別 worker のロールバックが完了するまでの短い間は予約が取れないので、有界に再試行する。
    async fn cycle_until_it_runs(&self) {
        for _ in 0..60 {
            if self
                .module
                .run_registration_enforcement_probe(INTERVAL)
                .await
                .unwrap()
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the probe cycle could not be reserved after the interruption");
    }

    /// 健全な初回サイクルで verified な基準状態を作り、その時点の通知イベント数を返す。
    async fn baseline(&self) -> (i64, usize) {
        self.cycle_until_it_runs().await;
        assert!(self.evidence().await.verified);
        assert!(self.enforcement_verified().await);
        (self.event_count().await, self.canaries().await.len())
    }

    async fn wait_entered(&self) {
        tokio::time::timeout(Duration::from_secs(10), self.kratos.entered.notified())
            .await
            .expect("the cycle reached the stall point");
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn probe_cycle_interrupted_at_any_stage_leaves_no_fabricated_notification_and_converges_after_restart(
) {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    for stall in [Stall::Initialize, Stall::Submit, Stall::Cleanup] {
        let fx = Fixture::new().await;
        let (events_before, canaries_before) = fx.baseline().await;
        let evidence_before = fx.evidence().await;
        fx.make_due().await;
        fx.set_stall(Some(stall));

        let cycle = {
            let module = fx.module.clone();
            tokio::spawn(async move { module.run_registration_enforcement_probe(INTERVAL).await })
        };
        fx.wait_entered().await;
        // worker の停止と同じ: 実行中のサイクルの future が drop される。
        cycle.abort();
        assert!(cycle.await.unwrap_err().is_cancelled());
        fx.set_stall(None);

        // 中断しただけでは通知を作らず、証拠も書き換えない (= 検証済みを偽装しない)。
        assert_eq!(fx.event_count().await, events_before, "{stall:?}");
        let evidence = fx.evidence().await;
        assert_eq!(evidence.checked_at, evidence_before.checked_at, "{stall:?}");
        assert!(evidence.verified);

        // 準備済みの canary は失われない (準備前の中断では増えない)。
        let canaries = fx.canaries().await;
        let aborted: Vec<_> = canaries[canaries_before..].to_vec();
        match stall {
            Stall::Initialize => assert!(aborted.is_empty(), "{aborted:?}"),
            Stall::Submit => assert_eq!(aborted.len(), 1, "{aborted:?}"),
            Stall::Cleanup => assert_eq!(aborted.len(), 1, "{aborted:?}"),
        }
        for (_, settled, _) in &aborted {
            assert!(
                !settled,
                "an interrupted canary is never settled as confirmed"
            );
        }

        // 再起動後のサイクルは (予約が rollback されているので) すぐ走り、収束する。
        fx.cycle_until_it_runs().await;
        let evidence = fx.evidence().await;
        assert!(evidence.verified, "{stall:?}");
        assert!(
            evidence.checked_at > evidence_before.checked_at,
            "{stall:?}"
        );
        assert!(fx.enforcement_verified().await, "{stall:?}");
        assert_eq!(
            fx.event_count().await,
            events_before,
            "no duplicate or fabricated notification: {stall:?}"
        );

        // 中断された canary は、再起動後に Kratos 側の取り残し (作られた identity) が無いか確認される。
        for (probe_id, _, _) in &aborted {
            let email = EnforcementProbeId::parse(probe_id).unwrap().email();
            assert!(
                fx.kratos.cleaned_emails.lock().unwrap().contains(&email),
                "the interrupted canary was verified clean after restart: {stall:?}"
            );
        }
        // どの canary も 1 回しか送られない (重複した probe を作らない)。
        let mut ids = fx.kratos.submitted_probe_ids.lock().unwrap().clone();
        let submitted = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), submitted, "{stall:?}");
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn probe_worker_loop_returns_on_shutdown_mid_cycle_and_a_restarted_loop_converges() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let fx = Fixture::new().await;
    let (events_before, canaries_before) = fx.baseline().await;
    let evidence_before = fx.evidence().await;
    fx.make_due().await;
    fx.set_stall(Some(Stall::Submit));

    let (tx, rx) = watch::channel(false);
    let worker = tokio::spawn(probe::run(fx.module.clone(), rx));
    fx.wait_entered().await;
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("the probe loop returns promptly on shutdown")
        .unwrap();
    // 停止後は次の tick を実行しない。
    let submitted = fx.kratos.submitted_probe_ids.lock().unwrap().len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        fx.kratos.submitted_probe_ids.lock().unwrap().len(),
        submitted
    );
    assert_eq!(fx.event_count().await, events_before);
    assert_eq!(fx.canaries().await.len(), canaries_before + 1);
    assert_eq!(fx.evidence().await.checked_at, evidence_before.checked_at);

    // 再起動したループが収束する。
    fx.set_stall(None);
    fx.make_due().await;
    let (tx, rx) = watch::channel(false);
    let worker = tokio::spawn(probe::run(fx.module.clone(), rx));
    let mut converged = false;
    for _ in 0..200 {
        if fx.evidence().await.checked_at > evidence_before.checked_at {
            converged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(converged, "the restarted loop ran a cycle");
    assert!(fx.evidence().await.verified);
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("the restarted loop stops")
        .unwrap();
    assert_eq!(fx.event_count().await, events_before);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn protection_maintenance_loop_returns_on_shutdown_and_does_not_tick_afterwards() {
    let _guard = GATE_ROUTE_TEST_LOCK.lock().await;
    let fx = Fixture::new().await;
    let events_before = fx.event_count().await;

    let (tx, rx) = watch::channel(false);
    let worker = tokio::spawn(worker::run(fx.module.clone(), Duration::from_secs(1), rx));
    tokio::time::sleep(Duration::from_millis(400)).await;
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .expect("the maintenance loop returns promptly on shutdown")
        .unwrap();
    let next_run: time::OffsetDateTime = fx
        .sql_scalar("SELECT next_run_at FROM registration_protection_schedule WHERE id = 1")
        .await;
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let after: time::OffsetDateTime = fx
        .sql_scalar("SELECT next_run_at FROM registration_protection_schedule WHERE id = 1")
        .await;
    assert_eq!(after, next_run, "no maintenance cycle ran after the stop");
    assert_eq!(fx.event_count().await, events_before);
}
