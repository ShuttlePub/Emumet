use super::support::{Env, User, LOCAL_BASE};
use crate::dto::profile_relationship::ProfileFollowDto;
use crate::service::profile_relationship::ProfileRelationshipUseCase;
use driver::http_signing::HttpSignatureVerifierImpl;
use driver::profile_delivery::ProfileDeliveryNetworkSettings;
use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::database::{DependOnTransactionManager, TransactionManager};
use kernel::interfaces::http_signing::{
    HttpSignatureVerificationInput, HttpSignatureVerifier, SignatureVerificationResult,
};
use kernel::interfaces::repository::{
    DependOnProfileDeliveryEnqueuer, DependOnProfileDeliveryJobRepository, ProfileDeliveryEnqueuer,
    ProfileDeliveryJobRepository,
};
use kernel::prelude::entity::{
    DeliveryActivityType, DeliveryClaimPolicy, DeliveryFailureReason, DeliveryResultClass,
    GraphActivityId, GraphInboxUrl, NewProfileDeliveryJob, ProfileDeliveryJobState,
    ProfileDeliveryPolicy,
};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration as StdDuration;
use time::{Duration, OffsetDateTime};
use wiremock::ResponseTemplate;

struct Setup {
    alice: User,
    follow: ProfileFollowDto,
    public_key: String,
}

/// 署名鍵を持つ Profile が peer の actor を Follow した状態 (Follow job が 1 件 pending)。
async fn follow_setup(env: &Env) -> Setup {
    let alice = env.seed_user().await;
    let public_key = env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.mount_actor("/users/bob").await;
    let follow = env
        .module
        .follow_profile(
            &alice.auth,
            None,
            &alice.nanoid,
            &env.peer_url("/users/bob"),
        )
        .await
        .unwrap();
    Setup {
        alice,
        follow,
        public_key,
    }
}

async fn verify(
    env: &Env,
    request: &wiremock::Request,
    key_id: &str,
    public_key: &str,
    body: Vec<u8>,
) -> SignatureVerificationResult {
    let mut verifier = HttpSignatureVerifierImpl::new().unwrap();
    verifier.cache_actor_key(key_id, public_key.to_string());
    let headers: HashMap<String, String> = request
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap().to_string(),
            )
        })
        .collect();
    verifier
        .verify(&HttpSignatureVerificationInput {
            method: "POST".to_string(),
            url: env.peer_url("/inbox"),
            headers,
            body: Some(body),
        })
        .await
        .unwrap()
}

async fn attempts(env: &Env, setup: &Setup) -> Vec<(i32, DeliveryResultClass, Option<i32>)> {
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    let mut executor = env.module.database_connection().connection().await.unwrap();
    env.module
        .profile_delivery_job_repository()
        .list_attempts(&mut executor, &job.id)
        .await
        .unwrap()
        .into_iter()
        .map(|a| (a.attempt_no, a.result, a.http_status))
        .collect()
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_follow_is_delivered_with_a_verifiable_profile_signature() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(202)).await;

    let report = env.cycle("worker-1").await;

    assert_eq!((report.attempted, report.delivered), (1, 1));
    let requests = env.inbox_requests().await;
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    let key_id = format!("{LOCAL_BASE}/ap/profiles/{}#main-key", setup.alice.nanoid);
    assert_eq!(
        verify(
            &env,
            request,
            &key_id,
            &setup.public_key,
            request.body.clone()
        )
        .await,
        SignatureVerificationResult::Valid {
            key_id: key_id.clone()
        }
    );
    assert!(
        matches!(
            verify(
                &env,
                request,
                &key_id,
                &setup.public_key,
                b"{\"tampered\":true}".to_vec()
            )
            .await,
            SignatureVerificationResult::Invalid(_)
        ),
        "the signature covers the exact body bytes"
    );
    assert_eq!(
        request
            .headers
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/activity+json"
    );
    let activity: Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(activity["type"], "Follow");
    assert_eq!(
        activity["actor"],
        format!("{LOCAL_BASE}/ap/profiles/{}", setup.alice.nanoid)
    );
    assert_eq!(activity["object"], env.peer_url("/users/bob"));
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(activity["id"], job.activity_id.as_str());
    assert_eq!(job.payload.as_bytes(), request.body.as_slice());
    assert_eq!(job.state, ProfileDeliveryJobState::Delivered);
    assert_eq!(job.attempt_count, 1);
    assert_eq!(
        attempts(&env, &setup).await,
        vec![(1, DeliveryResultClass::Delivered, Some(202))]
    );

    // 配送済みの job は再配送されない。
    assert_eq!(env.cycle("worker-1").await.attempted, 0);
    assert_eq!(env.inbox_requests().await.len(), 1);
    let _ = setup.follow;
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_server_error_is_retried_with_backoff_and_a_later_attempt_delivers() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(503)).await;

    let before = env.now();
    let report = env.cycle("worker-1").await;

    assert_eq!((report.attempted, report.retrying), (1, 1));
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Retrying);
    assert_eq!(job.attempt_count, 1);
    assert_eq!(job.last_result, Some(DeliveryResultClass::HttpServerError));
    assert_eq!(job.last_http_status, Some(503));
    let delay = job.next_attempt_at - before;
    assert!(
        delay >= Duration::seconds(59) && delay <= Duration::seconds(75),
        "first backoff is the base delay (60s), got {delay}"
    );

    assert_eq!(env.cycle("worker-1").await.attempted, 0, "not due yet");
    env.advance_clock(Duration::seconds(61));
    env.respond_inbox(ResponseTemplate::new(202)).await;
    let report = env.cycle("worker-1").await;

    assert_eq!(report.delivered, 1);
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Delivered);
    assert_eq!(job.attempt_count, 2);
    assert_eq!(
        attempts(&env, &setup).await,
        vec![
            (1, DeliveryResultClass::HttpServerError, Some(503)),
            (2, DeliveryResultClass::Delivered, Some(202))
        ]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_peer_timeout_is_a_retryable_failure() {
    let env = Env::with_sender_settings(ProfileDeliveryNetworkSettings {
        total_timeout: StdDuration::from_millis(300),
        ..ProfileDeliveryNetworkSettings::default()
    })
    .await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(202).set_delay(StdDuration::from_secs(3)))
        .await;

    let report = env.cycle("worker-1").await;

    assert_eq!(report.retrying, 1);
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Retrying);
    assert_eq!(job.last_result, Some(DeliveryResultClass::Timeout));
    assert_eq!(job.last_http_status, None);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_permanent_rejection_fails_without_retrying() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(410)).await;

    let report = env.cycle("worker-1").await;

    assert_eq!(report.failed, 1);
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Failed);
    assert_eq!(
        job.failure_reason,
        Some(DeliveryFailureReason::PermanentError)
    );
    env.advance_clock(Duration::days(2));
    assert_eq!(env.cycle("worker-1").await.attempted, 0);
    assert_eq!(env.inbox_requests().await.len(), 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retries_stop_at_the_attempt_limit() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    let policy = ProfileDeliveryPolicy {
        max_attempts: 2,
        ..ProfileDeliveryPolicy::default()
    };
    env.respond_inbox(ResponseTemplate::new(500)).await;

    assert_eq!(env.cycle_with("w", &policy).await.retrying, 1);
    env.advance_clock(Duration::hours(1));
    assert_eq!(env.cycle_with("w", &policy).await.failed, 1);

    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Failed);
    assert_eq!(
        job.failure_reason,
        Some(DeliveryFailureReason::RetriesExhausted)
    );
    assert_eq!(job.attempt_count, 2);
    env.advance_clock(Duration::days(2));
    assert_eq!(env.cycle_with("w", &policy).await.attempted, 0);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn retry_after_from_the_peer_is_honored() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(429).insert_header("Retry-After", "900"))
        .await;

    let before = env.now();
    env.cycle("worker-1").await;

    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(
        job.last_result,
        Some(DeliveryResultClass::HttpTooManyRequests)
    );
    assert!(job.next_attempt_at - before >= Duration::seconds(895));
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_job_left_in_flight_by_a_stopped_worker_is_recovered_after_its_lease_expires() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    // worker が job を取得した直後に落ちた状態 (結果は記録されない)。
    {
        let mut executor = env.module.database_connection().connection().await.unwrap();
        let claimed = env
            .module
            .profile_delivery_job_repository()
            .claim_due(
                &mut executor,
                "crashed-worker",
                env.now(),
                &DeliveryClaimPolicy {
                    lease_duration: Duration::seconds(60),
                    limit: 1,
                    max_attempts: 12,
                },
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
    }
    env.respond_inbox(ResponseTemplate::new(202)).await;

    assert_eq!(
        env.cycle("restarted-worker").await.attempted,
        0,
        "the crashed worker's lease is still valid"
    );
    env.advance_clock(Duration::seconds(61));
    let report = env.cycle("restarted-worker").await;

    assert_eq!(report.delivered, 1);
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Delivered);
    assert_eq!(job.attempt_count, 2);
    assert_eq!(
        attempts(&env, &setup).await,
        vec![
            (1, DeliveryResultClass::LeaseExpired, None),
            (2, DeliveryResultClass::Delivered, Some(202))
        ]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_job_whose_lease_keeps_expiring_ends_as_retries_exhausted() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    let policy = ProfileDeliveryPolicy {
        max_attempts: 2,
        ..ProfileDeliveryPolicy::default()
    };
    for round in 0..2 {
        let mut executor = env.module.database_connection().connection().await.unwrap();
        let claimed = env
            .module
            .profile_delivery_job_repository()
            .claim_due(
                &mut executor,
                &format!("crashed-{round}"),
                env.now(),
                &DeliveryClaimPolicy {
                    lease_duration: Duration::seconds(60),
                    limit: 1,
                    max_attempts: policy.max_attempts,
                },
            )
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        drop(executor);
        env.advance_clock(Duration::seconds(61));
    }

    env.respond_inbox(ResponseTemplate::new(202)).await;
    assert_eq!(env.cycle_with("w", &policy).await.attempted, 0);

    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Failed);
    assert_eq!(
        job.failure_reason,
        Some(DeliveryFailureReason::RetriesExhausted)
    );
    assert!(env.inbox_requests().await.is_empty());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_workers_never_deliver_the_same_job_twice() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.create_key(&alice.profile_id, &alice.nanoid).await;
    let jobs: Vec<NewProfileDeliveryJob> = (0..8)
        .map(|n| {
            NewProfileDeliveryJob::new(
                alice.profile_id.clone(),
                DeliveryActivityType::Follow,
                GraphActivityId::new(format!(
                    "{LOCAL_BASE}/ap/profiles/{}/activities/{n}",
                    alice.nanoid
                ))
                .unwrap(),
                GraphInboxUrl::new(env.peer_url("/inbox")).unwrap(),
                format!("{{\"type\":\"Follow\",\"n\":{n}}}"),
                OffsetDateTime::now_utc(),
            )
            .unwrap()
        })
        .collect();
    {
        let deps = env.module.clone();
        env.module
            .transaction_manager()
            .transaction(move |executor| {
                Box::pin(async move {
                    for job in &jobs {
                        deps.profile_delivery_enqueuer()
                            .enqueue(executor, job)
                            .await?;
                    }
                    Ok(())
                })
            })
            .await
            .unwrap();
    }
    env.respond_inbox(ResponseTemplate::new(202).set_delay(StdDuration::from_millis(40)))
        .await;

    let (a, b) = tokio::join!(env.cycle("worker-a"), env.cycle("worker-b"));

    assert_eq!(a.delivered + b.delivered, 8, "{a:?} {b:?}");
    assert!(
        a.attempted > 0 && b.attempted > 0,
        "both workers took part: {a:?} {b:?}"
    );
    let mut seen: Vec<i64> = env
        .inbox_requests()
        .await
        .iter()
        .map(|request| {
            serde_json::from_slice::<Value>(&request.body).unwrap()["n"]
                .as_i64()
                .unwrap()
        })
        .collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..8).collect::<Vec<i64>>(),
        "every activity reached the peer exactly once"
    );
    for job in env.jobs_of(&alice.profile_id).await {
        assert_eq!(job.state, ProfileDeliveryJobState::Delivered);
        assert_eq!(job.attempt_count, 1);
    }
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_cancelled_job_is_not_delivered_and_a_cancel_during_sending_is_recorded() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(202)).await;
    let job_id = env.jobs_of(&setup.alice.profile_id).await[0].id.clone();
    {
        let mut executor = env.module.database_connection().connection().await.unwrap();
        assert!(env
            .module
            .profile_delivery_job_repository()
            .cancel(&mut executor, &job_id, "test", OffsetDateTime::now_utc())
            .await
            .unwrap());
    }
    assert_eq!(env.cycle("w").await.attempted, 0);
    assert!(env.inbox_requests().await.is_empty());

    // 送信中の取消: 履歴だけが残り、状態は cancelled のまま。
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(202).set_delay(StdDuration::from_millis(800)))
        .await;
    let job_id = env.jobs_of(&setup.alice.profile_id).await[0].id.clone();
    let canceller = async {
        tokio::time::sleep(StdDuration::from_millis(250)).await;
        let mut executor = env.module.database_connection().connection().await.unwrap();
        env.module
            .profile_delivery_job_repository()
            .cancel(
                &mut executor,
                &job_id,
                "unfollowed while sending",
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap()
    };
    let (report, cancelled) = tokio::join!(env.cycle("w"), canceller);

    assert!(cancelled);
    assert_eq!(
        (report.attempted, report.cancelled, report.delivered),
        (1, 1, 0)
    );
    let job = env.job(&job_id).await;
    assert_eq!(job.state, ProfileDeliveryJobState::Cancelled);
    assert_eq!(job.attempt_count, 1);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_missing_signing_key_delays_the_delivery_without_sending_anything() {
    let env = Env::new().await;
    let alice = env.seed_user().await;
    env.mount_actor("/users/bob").await;
    env.module
        .follow_profile(
            &alice.auth,
            None,
            &alice.nanoid,
            &env.peer_url("/users/bob"),
        )
        .await
        .unwrap();
    env.respond_inbox(ResponseTemplate::new(202)).await;

    let report = env.cycle("w").await;

    assert_eq!((report.attempted, report.retrying), (1, 1));
    assert!(
        env.inbox_requests().await.is_empty(),
        "an unsigned request is never sent"
    );
    let job = &env.jobs_of(&alice.profile_id).await[0];
    assert_eq!(job.last_result, Some(DeliveryResultClass::SigningFailed));

    env.create_key(&alice.profile_id, &alice.nanoid).await;
    env.advance_clock(Duration::seconds(61));
    assert_eq!(env.cycle("w").await.delivered, 1);
    assert_eq!(env.inbox_requests().await.len(), 1);
}

/// 署名鍵の読み取りを止める (`profile_signing_keys` を排他ロックした transaction を保持する)。
async fn stall_key_reads(env: &Env) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut tx = env.iso.pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE profile_signing_keys IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_worker_that_lost_its_lease_while_signing_does_not_send() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(202)).await;
    let key_lock = stall_key_reads(&env).await;

    // worker-a は job を取得した後、鍵の取得で止まる。その間に lease が切れ、worker-b が回収する
    // (worker-b も鍵の取得で止まる)。鍵の取得が再開すると、worker-a は lease を失っているので送信しない。
    let worker_a = env.cycle("worker-a");
    let worker_b = async {
        tokio::time::sleep(StdDuration::from_millis(400)).await;
        env.advance_clock(Duration::seconds(121));
        env.cycle("worker-b").await
    };
    let release = async {
        tokio::time::sleep(StdDuration::from_millis(1200)).await;
        key_lock.commit().await.unwrap();
    };
    let (a, b, ()) = tokio::join!(worker_a, worker_b, release);

    assert_eq!((a.lease_lost, a.delivered), (1, 0), "{a:?}");
    assert_eq!(b.delivered, 1, "{b:?}");
    assert_eq!(
        env.inbox_requests().await.len(),
        1,
        "the activity reached the peer exactly once"
    );
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert_eq!(job.state, ProfileDeliveryJobState::Delivered);
    assert_eq!(job.attempt_count, 2);
    assert_eq!(
        attempts(&env, &setup).await,
        vec![
            (1, DeliveryResultClass::LeaseExpired, None),
            (2, DeliveryResultClass::Delivered, Some(202))
        ]
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn a_job_cancelled_while_the_worker_was_signing_is_not_sent() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    env.respond_inbox(ResponseTemplate::new(202)).await;
    let job_id = env.jobs_of(&setup.alice.profile_id).await[0].id.clone();
    let key_lock = stall_key_reads(&env).await;

    let worker = env.cycle("worker-a");
    let canceller = async {
        tokio::time::sleep(StdDuration::from_millis(400)).await;
        let mut executor = env.module.database_connection().connection().await.unwrap();
        env.module
            .profile_delivery_job_repository()
            .cancel(
                &mut executor,
                &job_id,
                "unfollowed",
                OffsetDateTime::now_utc(),
            )
            .await
            .unwrap()
    };
    let release = async {
        tokio::time::sleep(StdDuration::from_millis(900)).await;
        key_lock.commit().await.unwrap();
    };
    let (report, cancelled, ()) = tokio::join!(worker, canceller, release);

    assert!(cancelled);
    assert_eq!((report.cancelled, report.delivered), (1, 0), "{report:?}");
    assert!(
        env.inbox_requests().await.is_empty(),
        "a cancelled job is never sent"
    );
    assert_eq!(
        env.job(&job_id).await.state,
        ProfileDeliveryJobState::Cancelled
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn renew_lease_extends_only_a_lease_that_is_still_held() {
    let env = Env::new().await;
    let setup = follow_setup(&env).await;
    let repository = env.module.profile_delivery_job_repository();
    let mut executor = env.module.database_connection().connection().await.unwrap();
    let t0 = env.now();
    let claim = repository
        .claim_due(
            &mut executor,
            "worker-a",
            t0,
            &DeliveryClaimPolicy {
                lease_duration: Duration::seconds(60),
                limit: 1,
                max_attempts: 12,
            },
        )
        .await
        .unwrap()
        .remove(0);

    let mut forged = claim.clone();
    forged.lease_token = "someone-elses-token".to_string();
    assert!(!repository
        .renew_lease(
            &mut executor,
            &forged,
            t0 + Duration::seconds(10),
            Duration::seconds(60)
        )
        .await
        .unwrap());
    assert!(repository
        .renew_lease(
            &mut executor,
            &claim,
            t0 + Duration::seconds(30),
            Duration::seconds(60)
        )
        .await
        .unwrap());
    let job = &env.jobs_of(&setup.alice.profile_id).await[0];
    assert!(
        job.lease_expires_at.unwrap() >= t0 + Duration::seconds(89),
        "the lease now runs to about t0 + 90s"
    );
    assert!(
        !repository
            .renew_lease(
                &mut executor,
                &claim,
                t0 + Duration::seconds(200),
                Duration::seconds(60)
            )
            .await
            .unwrap(),
        "an expired lease cannot be revived"
    );
}
