//! 保護 maintenance (worker の 1 サイクル) を任意の時点で打ち切って (= worker の停止) から再実行しても、
//! 通知イベントの重複・消失・捏造がなく、中断なしで実行した場合と同じ状態に収束することを、
//! 中断位置を掃引して固定する。実 PostgreSQL + fake provider。

use super::*;

fn timeout_fault(at: OffsetDateTime) -> ProviderResult {
    ProviderResult::new(
        ProviderOutcome::TemporarilyUnavailable,
        ReasonCode::Timeout,
        TransportClass::Timeout,
    )
    .at_stage(CheckStage::Connect)
    .observed(at)
}

async fn make_due(f: &Fixture) {
    let mut connection = f.module.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut connection;
    sqlx::query(
        "UPDATE registration_protection_schedule SET next_run_at = now() - interval '1 second'",
    )
    .execute(con)
    .await
    .unwrap();
}

/// 運用可能な状態から、bot provider の probe だけが失敗する状況にする (中断で途中の状態が見えても、
/// 通知が増えない単純な遷移: 防御低下 1 件)。
async fn faulty_providers(f: &Fixture) {
    f.ready_for_issuing().await;
    let at = now();
    *f.module.turnstile.probe.lock().unwrap() = timeout_fault(at);
    make_due(f).await;
}

/// 通知イベントの (種別, 件数) と、メール・bot の観測。
async fn outcome(f: &Fixture) -> (Vec<(String, i64)>, String) {
    let mut connection = f.module.db.connection().await.unwrap();
    let con: &mut PgConnection = &mut connection;
    let events: Vec<(String, i64)> = sqlx::query_as(
        "SELECT kind, count(*) FROM registration_gate_events GROUP BY kind ORDER BY kind",
    )
    .fetch_all(con)
    .await
    .unwrap();
    drop(connection);
    let observation = format!("{:?}", f.gate().await.observation);
    (events, observation)
}

/// 中断の後始末 (別接続の rollback) が終わるまでの短い間は予約が取れないので、有界に再実行する。
async fn rerun_until_settled(f: &Fixture) {
    for _ in 0..60 {
        make_due(f).await;
        if f.module
            .run_registration_protection_maintenance(Duration::seconds(30))
            .await
            .unwrap()
        {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("maintenance could not be reserved again after the interruption");
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn maintenance_interrupted_at_any_point_neither_duplicates_nor_loses_notifications() {
    let reference = {
        let f = Fixture::new().await;
        faulty_providers(&f).await;
        assert!(f
            .module
            .run_registration_protection_maintenance(Duration::seconds(30))
            .await
            .unwrap());
        outcome(&f).await
    };
    assert!(
        !reference.0.is_empty(),
        "the scenario must produce at least one notification: {reference:?}"
    );

    let mut interrupted_runs = 0;
    for micros in [
        0u64, 100, 300, 600, 1_000, 1_500, 2_000, 3_000, 4_500, 6_500, 9_000, 13_000, 20_000,
        40_000,
    ] {
        let f = Fixture::new().await;
        faulty_providers(&f).await;
        let cut = tokio::time::timeout(
            std::time::Duration::from_micros(micros),
            f.module
                .run_registration_protection_maintenance(Duration::seconds(30)),
        )
        .await;
        if cut.is_err() {
            interrupted_runs += 1;
        }
        // 中断の直後に、すでに通知が重複していない。
        let (events, _) = outcome(&f).await;
        assert!(
            events.iter().all(|(_, count)| *count == 1),
            "{micros}us: {events:?}"
        );
        rerun_until_settled(&f).await;
        assert_eq!(
            outcome(&f).await,
            reference,
            "cut after {micros}us must converge to the uninterrupted result"
        );
    }
    assert!(
        interrupted_runs > 0,
        "at least one cut must actually interrupt a cycle for the sweep to mean anything"
    );
}
