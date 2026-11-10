//! Profile 配送 worker の 1 サイクル (P3o)。
//!
//! 永続化された配送 job (`profile_delivery_jobs`) を lease つきで 1 件ずつ取得し、Profile 鍵で HTTP 署名
//! して inbox へ送信し、結果 (配送済み / 再試行 / 失敗) を記録する。
//!
//! - **lease は送信の直前に 1 件ずつ**取る (バッチで取ると後ろの job の lease が前の送信中に切れ、
//!   別 worker が同じ job を同時に送ってしまう)。送信中は DB 接続を保持しない。
//! - 二重 worker は `claim_due` の `FOR UPDATE SKIP LOCKED` と `record_attempt` の `lease_token` fencing で
//!   同じ job を二重に配送しない。lease を失った結果は記録しない (`lease_lost`)。
//! - worker が送信中に止まると lease が切れ、次のサイクルが `LeaseExpired` として回収して再試行する
//!   (再起動後の回収)。`max_attempts` に達した job は `failed(retries_exhausted)`。
//! - claim から送信までに時間がかかる (鍵の取得・署名が詰まる) 場合に備え、**署名の後・送信の直前に
//!   `renew_lease` で lease を再確認して延長する**。取消・期限切れ・他 worker による回収があれば送信しない。
//!   送信自体も延長後の lease より短い時間 (残存 lease から安全マージンを引いた時間) に制限するので、
//!   lease を失った worker が送信を続けることはない。
//! - 取り消された job は取得されない。送信中に取り消された場合は履歴だけ残る (`cancelled`)。
//! - 本文は job に保存されたバイト列そのもの。`Digest` と HTTP 署名はこのバイト列から作る。
//!   署名は Profile 鍵 (`SignProfileRequestUseCase`) で、新しい汎用署名 API は作らない。
//!
//! 本番の root から起動しない (P4 が所有)。

use crate::profile_signing_key::SignProfileRequestUseCase;
use base64::{engine::general_purpose, Engine as _};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::http_signing::HttpSigningRequest;
use kernel::interfaces::repository::{
    DependOnProfileDeliveryJobRepository, DependOnProfileDeliverySender,
    ProfileDeliveryJobRepository, ProfileDeliveryRequest, ProfileDeliverySender,
};
use kernel::prelude::entity::{
    decide_profile_delivery, ClaimedProfileDeliveryJob, DeliveryAttemptOutcome,
    DeliveryClaimPolicy, ProfileDeliveryJobState, ProfileDeliveryPolicy, ProfileSendObservation,
    RecordAttemptOutcome,
};
use kernel::KernelError;
use sha2::Digest;
use std::collections::HashMap;
use std::future::Future;
use time::OffsetDateTime;

const ACTIVITY_JSON: &str = "application/activity+json";

/// lease を使い切る前に送信を打ち切るための安全マージン (これを引いた時間が送信の上限)。
const LEASE_SAFETY_MARGIN: time::Duration = time::Duration::seconds(10);

/// 署名・送信 1 件の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileDeliveryStep {
    /// 送信した (または宛先検査・署名で送信に至らなかった) 結果の観測。
    Observed(ProfileSendObservation),
    /// 送信の直前に job が取り消されていた。送信していない。
    Cancelled,
    /// 送信の直前に lease を失っていた (期限切れ・他 worker による回収)。送信していない。
    LeaseLost,
}

/// 時刻・乱数・停止判定の注入点。
pub struct ProfileDeliveryEnv<'a> {
    pub clock: &'a (dyn Fn() -> OffsetDateTime + Sync),
    /// `[0, 1)` の一様乱数 (バックオフの jitter)。
    pub jitter: &'a (dyn Fn() -> f64 + Sync),
    /// true なら次の job に進まず終了する (cooperative shutdown)。
    pub should_stop: &'a (dyn Fn() -> bool + Sync),
}

/// 1 サイクルの結果。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileDeliveryCycleReport {
    pub attempted: u32,
    pub delivered: u32,
    pub retrying: u32,
    pub failed: u32,
    /// 送信中に job が取り消された。
    pub cancelled: u32,
    /// 送信中に lease を失い結果を記録しなかった。
    pub lease_lost: u32,
}

fn host_header(url: &reqwest::Url) -> Option<String> {
    let host = url.host_str()?;
    Some(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

pub trait DeliverProfileActivitiesUseCase:
    SignProfileRequestUseCase + DependOnProfileDeliveryJobRepository + DependOnProfileDeliverySender
{
    /// 取得した job を 1 件署名・送信する。結果は分類済みの観測として返す (再試行の判断は呼び出し側)。
    fn send_claimed_profile_delivery<'a>(
        &'a self,
        claim: &'a ClaimedProfileDeliveryJob,
        policy: &'a ProfileDeliveryPolicy,
        clock: &'a (dyn Fn() -> OffsetDateTime + Sync),
    ) -> impl Future<Output = error_stack::Result<ProfileDeliveryStep, KernelError>> + Send + 'a
    {
        async move {
            let Ok(url) = reqwest::Url::parse(claim.target_inbox.as_str()) else {
                return Ok(ProfileDeliveryStep::Observed(
                    ProfileSendObservation::InvalidTarget,
                ));
            };
            let Some(host) = host_header(&url) else {
                return Ok(ProfileDeliveryStep::Observed(
                    ProfileSendObservation::InvalidTarget,
                ));
            };
            let body = claim.payload.as_bytes().to_vec();
            let digest = format!(
                "SHA-256={}",
                general_purpose::STANDARD.encode(sha2::Sha256::digest(&body))
            );
            let date = httpdate::fmt_http_date(std::time::SystemTime::now());
            let mut signed_headers = HashMap::new();
            signed_headers.insert("host".to_string(), host);
            signed_headers.insert("date".to_string(), date.clone());
            signed_headers.insert("digest".to_string(), digest.clone());
            signed_headers.insert("content-type".to_string(), ACTIVITY_JSON.to_string());
            let signature = match self
                .sign_as_profile(
                    &claim.sender,
                    HttpSigningRequest {
                        method: "POST".to_string(),
                        url: claim.target_inbox.as_str().to_string(),
                        headers: signed_headers,
                        body: Some(body.clone()),
                    },
                )
                .await
            {
                Ok(signature) => signature,
                Err(error) => {
                    tracing::warn!(error = ?error, "profile delivery signing failed");
                    return Ok(ProfileDeliveryStep::Observed(
                        ProfileSendObservation::SigningFailed,
                    ));
                }
            };
            let mut headers = vec![
                ("date".to_string(), date),
                ("digest".to_string(), digest),
                ("content-type".to_string(), ACTIVITY_JSON.to_string()),
            ];
            for (name, value) in &signature.cavage_headers {
                let lower = name.to_ascii_lowercase();
                if matches!(lower.as_str(), "host" | "date" | "digest" | "content-type") {
                    continue;
                }
                headers.push((lower, value.clone()));
            }
            // 署名までに時間が経っていても、送信の直前に lease を再確認して延長する。取消・期限切れ・
            // 他 worker による回収があれば送信しない。
            let renewed_at = clock();
            let renewed = {
                let mut connection = self.database_connection().connection().await?;
                self.profile_delivery_job_repository()
                    .renew_lease(&mut connection, claim, renewed_at, policy.lease_duration)
                    .await?
            };
            if !renewed {
                let mut connection = self.database_connection().connection().await?;
                let cancelled = self
                    .profile_delivery_job_repository()
                    .find_by_id(&mut connection, &claim.id)
                    .await?
                    .is_some_and(|job| job.state == ProfileDeliveryJobState::Cancelled);
                return Ok(if cancelled {
                    ProfileDeliveryStep::Cancelled
                } else {
                    ProfileDeliveryStep::LeaseLost
                });
            }
            // 送信は、更新に使った時刻から決まる絶対期限 (延長後の lease の終わりから安全マージンを引いた
            // 時刻) までに制限する。更新の SQL がロック待ちなどで遅れて期限・マージンを使い切っていたら
            // 送信しない。送信が期限を超えたら timeout として再試行する。
            let margin = LEASE_SAFETY_MARGIN.min(policy.lease_duration / 2);
            let deadline = renewed_at + policy.lease_duration - margin;
            let remaining = deadline - clock();
            let Ok(budget) = std::time::Duration::try_from(remaining) else {
                return Ok(ProfileDeliveryStep::LeaseLost);
            };
            if budget.is_zero() {
                return Ok(ProfileDeliveryStep::LeaseLost);
            }
            let request = ProfileDeliveryRequest {
                url: claim.target_inbox.as_str(),
                headers: &headers,
                body: &body,
            };
            let send = self.profile_delivery_sender().send(&request);
            Ok(ProfileDeliveryStep::Observed(
                match tokio::time::timeout(budget, send).await {
                    Ok(observation) => observation,
                    Err(_) => ProfileSendObservation::Timeout,
                },
            ))
        }
    }

    /// 配送サイクル: (1 件ずつ) lease 取得 → 署名・送信 → 結果記録。最大 `policy.batch_size` 件を処理する。
    fn deliver_profile_activities<'a>(
        &'a self,
        worker_id: &'a str,
        policy: &'a ProfileDeliveryPolicy,
        env: &'a ProfileDeliveryEnv<'a>,
    ) -> impl Future<Output = error_stack::Result<ProfileDeliveryCycleReport, KernelError>> + Send + 'a
    {
        async move {
            let mut report = ProfileDeliveryCycleReport::default();
            let claim_policy = DeliveryClaimPolicy {
                lease_duration: policy.lease_duration,
                limit: 1,
                max_attempts: policy.max_attempts,
            };
            for _ in 0..policy.batch_size.max(1) {
                if (env.should_stop)() {
                    break;
                }
                let claim = {
                    let mut connection = self.database_connection().connection().await?;
                    self.profile_delivery_job_repository()
                        .claim_due(&mut connection, worker_id, (env.clock)(), &claim_policy)
                        .await?
                        .into_iter()
                        .next()
                };
                let Some(claim) = claim else {
                    break;
                };
                let observation = match self
                    .send_claimed_profile_delivery(&claim, policy, env.clock)
                    .await?
                {
                    ProfileDeliveryStep::Observed(observation) => observation,
                    ProfileDeliveryStep::Cancelled => {
                        report.attempted += 1;
                        report.cancelled += 1;
                        continue;
                    }
                    ProfileDeliveryStep::LeaseLost => {
                        report.attempted += 1;
                        report.lease_lost += 1;
                        continue;
                    }
                };
                let finished_at = (env.clock)();
                let outcome = decide_profile_delivery(
                    &observation,
                    claim.attempt_no,
                    finished_at,
                    policy,
                    (env.jitter)(),
                );
                let recorded = {
                    let mut connection = self.database_connection().connection().await?;
                    self.profile_delivery_job_repository()
                        .record_attempt(&mut connection, &claim, finished_at, &outcome)
                        .await?
                };
                report.attempted += 1;
                match recorded {
                    RecordAttemptOutcome::LeaseLost => report.lease_lost += 1,
                    RecordAttemptOutcome::Cancelled => report.cancelled += 1,
                    RecordAttemptOutcome::Recorded => match outcome {
                        DeliveryAttemptOutcome::Delivered { .. } => report.delivered += 1,
                        DeliveryAttemptOutcome::Retry { .. } => report.retrying += 1,
                        DeliveryAttemptOutcome::Failed { .. } => report.failed += 1,
                    },
                }
            }
            Ok(report)
        }
    }
}

impl<T> DeliverProfileActivitiesUseCase for T where
    T: SignProfileRequestUseCase
        + DependOnProfileDeliveryJobRepository
        + DependOnProfileDeliverySender
{
}
