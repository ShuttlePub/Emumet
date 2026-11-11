use application::service::registration_gate_webhook::WebhookEndpointList;
use kernel::interfaces::repository::WebhookTestOutcome;
use kernel::prelude::entity::{
    WebhookCategory, WebhookDeliveryCounts, WebhookEndpointId, WebhookEndpointView,
    WebhookEventType, WebhookFailedDelivery, WebhookFailedPage, WriteOnlySecret,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

/// 環境変数の送信先 (暗号鍵なしで従来どおり配送している 1 件。read-only) の ID。
pub const ENVIRONMENT_ENDPOINT_ID: &str = "environment";

pub fn endpoint_id_to_wire(id: WebhookEndpointId) -> String {
    if id.is_environment() {
        ENVIRONMENT_ENDPOINT_ID.to_string()
    } else {
        id.as_i64().to_string()
    }
}

pub fn parse_endpoint_id(raw: &str) -> Option<WebhookEndpointId> {
    let raw = raw.trim();
    if raw == ENVIRONMENT_ENDPOINT_ID {
        return Some(WebhookEndpointId::ENVIRONMENT);
    }
    raw.parse::<i64>()
        .ok()
        .filter(|value| *value > 0)
        .map(WebhookEndpointId::new)
}

/// 送る通知のカテゴリ (識別子は固定)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum WebhookCategoryId {
    /// 障害: `outage_started` / `protection_degraded`
    Outage,
    /// 復旧: `outage_recovered` / `enforcement_restored`
    Recovery,
    /// 自動停止: `protection_lost_stopped` / `enforcement_lost_stopped`
    AutoStop,
    /// 強制開放: `override_started` / `override_revoked`
    Override,
}

impl From<WebhookCategoryId> for WebhookCategory {
    fn from(id: WebhookCategoryId) -> Self {
        match id {
            WebhookCategoryId::Outage => WebhookCategory::Outage,
            WebhookCategoryId::Recovery => WebhookCategory::Recovery,
            WebhookCategoryId::AutoStop => WebhookCategory::AutoStop,
            WebhookCategoryId::Override => WebhookCategory::Override,
        }
    }
}

impl From<WebhookCategory> for WebhookCategoryId {
    fn from(category: WebhookCategory) -> Self {
        match category {
            WebhookCategory::Outage => WebhookCategoryId::Outage,
            WebhookCategory::Recovery => WebhookCategoryId::Recovery,
            WebhookCategory::AutoStop => WebhookCategoryId::AutoStop,
            WebhookCategory::Override => WebhookCategoryId::Override,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateWebhookEndpointRequest {
    /// 送信先 URL (HTTPS のみ・userinfo と fragment は不可・2048 文字以内)。登録時に宛先検査
    /// (SSRF 対策) を通す。応答には一切返らず、部分マスク形 (`masked_url`) だけが返る。
    pub url: String,
    /// 署名 secret (書き込み専用。16〜256 バイト)。暗号化して保存し、応答・ログ・監査・通知には出ない。
    #[schema(value_type = String, format = Password, write_only)]
    pub secret: WriteOnlySecret,
    /// 送る通知カテゴリ (1 つ以上)。
    pub categories: Vec<WebhookCategoryId>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateWebhookEndpointRequest {
    /// 取得時の revision (楽観ロック。不一致は 409)。成功のたびに増える。
    pub revision: i64,
    /// 省略 = 変更しない。URL は応答に返らないので、編集画面は空欄で開き、入力したときだけ置き換える。
    #[serde(default)]
    pub url: Option<String>,
    /// 省略 = 変更しない (書き込み専用)。
    #[serde(default)]
    #[schema(value_type = Option<String>, format = Password, write_only)]
    pub secret: Option<WriteOnlySecret>,
    /// 省略 = 変更しない。指定する場合は 1 つ以上。変更は変更後に発生したイベントにだけ効く。
    #[serde(default)]
    pub categories: Option<Vec<WebhookCategoryId>>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct DeleteWebhookEndpointRequest {
    /// 取得時の revision (楽観ロック。不一致は 409)。
    pub revision: i64,
}

#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct WebhookRetryRequest {
    /// 再試行する送信先の ID。省略すると全送信先の failed 配送すべてを再キューする。
    #[serde(default)]
    pub endpoint_id: Option<String>,
    /// 再試行する配送の event ID (通知イベント ID)。`endpoint_id` の指定が必要で、
    /// (送信先, event ID) の 1 件だけを再キューする。省略するとその送信先の failed すべて。
    #[serde(default)]
    pub event_id: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookRetryResponse {
    /// 再キューした failed 配送の件数。同じ event ID が再送される。
    pub requeued: u64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookDeliveryCountsResponse {
    pub pending: i64,
    pub in_flight: i64,
    pub retrying: i64,
    pub delivered: i64,
    pub failed: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookEndpointResponse {
    /// 送信先の ID (10 進文字列)。`environment` は環境変数の送信先 (read-only)。
    pub id: String,
    /// `api` (管理者が登録) / `environment_import` (環境変数の 1 件を取り込んだ) /
    /// `environment` (暗号鍵なしで環境変数のまま配送。API から編集・削除できない)。
    pub source: String,
    /// 一覧用の部分マスク URL (例 `hooks.example.net/••••/a8f2`)。host (ポートがあれば含む) と、
    /// path があるときは伏せ字 + 12 文字以上のトークンの末尾 4 文字だけ。全 URL・path・query・
    /// userinfo は返さない。
    pub masked_url: String,
    /// 送る通知カテゴリ。
    pub categories: Vec<WebhookCategoryId>,
    /// 楽観ロック用 revision。環境変数の送信先にはない (`null`)。
    pub revision: Option<i64>,
    /// 状態チップ: `healthy` (正常) / `failing` (配送失敗)。`failing` = 未解決の failed 配送がある、
    /// または復号不可・設定不備などの恒久的な理由がある。failed が再試行で解消されれば `healthy` に戻る。
    pub status: String,
    /// `failing` の理由: `failed_deliveries` / `secret_undecryptable` (secret の再入力で復旧) /
    /// `config_invalid` (環境変数の設定不備)。
    pub failing_reasons: Vec<String>,
    /// 未解決の failed 配送の件数。
    pub failed_count: i64,
    pub counts: WebhookDeliveryCountsResponse,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_success_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_failure_at: Option<OffsetDateTime>,
    /// secret が設定済みか (secret の値は返さない)。
    pub secret_configured: bool,
    /// false = API から編集・削除できない (環境変数の送信先)。
    pub editable: bool,
    /// 環境変数の送信先の設定不備の分類 (`invalid_url` / `unsupported_scheme` / ...)。
    pub config_issue: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub created_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookEndpointListResponse {
    pub endpoints: Vec<WebhookEndpointResponse>,
    /// どこかの送信先に未解決の failed 配送がある (管理画面のタブの印用)。
    pub has_failed_deliveries: bool,
    /// 暗号鍵がサーバー設定にあるか。false だと送信先の作成・secret 更新は拒否される。
    pub encryption_key_configured: bool,
    /// 登録できる送信先の上限件数。
    pub max_endpoints: i64,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookTestResponse {
    /// テスト用の合成 event ID (`test_` で始まる。通知イベント ID とは衝突しない)。
    pub event_id: String,
    /// 受信側が 2xx を返したら true。
    pub delivered: bool,
    /// 結果の分類 (`delivered` / `timeout` / `connect_failed` / `dns_failed` / `http_request_timeout` /
    /// `http_too_many_requests` / `http_server_error` / `http_redirect` / `http_client_error` /
    /// `http_unexpected_status` / `ssrf_blocked` / `config_invalid` / `secret_undecryptable`)。
    pub result: String,
    pub http_status: Option<u16>,
    #[serde(with = "time::serde::rfc3339")]
    pub attempted_at: OffsetDateTime,
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct WebhookFailuresQuery {
    /// 送信先 ID で絞り込む。
    pub endpoint_id: Option<String>,
    /// 1〜100 (既定 50)。
    pub limit: Option<i64>,
    /// 前ページの `next_cursor` (不透明な文字列)。これより古い failed を返す。
    pub before: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookFailedDeliveryResponse {
    pub endpoint_id: String,
    /// 通知イベント ID。Webhook の `X-Emumet-Event-Id` ヘッダーと同じ値 (再試行・再送で不変)。
    pub event_id: String,
    /// Webhook の `type` (`outage_started` / ... / `override_revoked`)。
    pub event_type: String,
    pub category: WebhookCategoryId,
    #[serde(with = "time::serde::rfc3339")]
    pub failed_at: OffsetDateTime,
    /// 最後の試行結果の分類 (secret・URL 全体を含まない固定語彙)。
    pub result: Option<String>,
    pub http_status: Option<u16>,
    /// `permanent_error` / `retries_exhausted`
    pub failure_reason: Option<String>,
    pub attempt_count: u32,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct WebhookFailedDeliveriesResponse {
    /// failed になった時刻の新しい順 (同時刻は送信先 ID・event ID の降順)。成功した配送は含まない。
    pub items: Vec<WebhookFailedDeliveryResponse>,
    /// 次ページがあるときだけ。`before` にそのまま渡す。件数に関わらず古い failed にも到達できる。
    pub next_cursor: Option<String>,
}

fn serde_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(value)) => value,
        _ => String::new(),
    }
}

impl From<&WebhookDeliveryCounts> for WebhookDeliveryCountsResponse {
    fn from(counts: &WebhookDeliveryCounts) -> Self {
        Self {
            pending: counts.pending,
            in_flight: counts.in_flight,
            retrying: counts.retrying,
            delivered: counts.delivered,
            failed: counts.failed,
        }
    }
}

impl From<&WebhookEndpointView> for WebhookEndpointResponse {
    fn from(view: &WebhookEndpointView) -> Self {
        Self {
            id: endpoint_id_to_wire(view.id),
            source: serde_str(&view.source),
            masked_url: view.masked_url.clone(),
            categories: view
                .categories
                .as_slice()
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            revision: view.revision,
            status: serde_str(&view.health),
            failing_reasons: view.failing_reasons.iter().map(serde_str).collect(),
            failed_count: view.failed_count,
            counts: (&view.summary.counts).into(),
            last_success_at: view.summary.last_success_at,
            last_failure_at: view.summary.last_failure_at,
            secret_configured: view.secret_configured,
            editable: view.editable(),
            config_issue: view.config_issue.as_ref().map(serde_str),
            created_at: view.created_at,
            updated_at: view.updated_at,
        }
    }
}

impl From<&WebhookEndpointList> for WebhookEndpointListResponse {
    fn from(list: &WebhookEndpointList) -> Self {
        Self {
            endpoints: list.endpoints.iter().map(Into::into).collect(),
            has_failed_deliveries: list.has_failed_deliveries,
            encryption_key_configured: list.encryption_key_configured,
            max_endpoints: list.max_endpoints,
        }
    }
}

impl From<&WebhookFailedDelivery> for WebhookFailedDeliveryResponse {
    fn from(delivery: &WebhookFailedDelivery) -> Self {
        Self {
            endpoint_id: endpoint_id_to_wire(delivery.endpoint_id),
            event_id: delivery.event_id.as_ref().to_string(),
            event_type: serde_str(&WebhookEventType::from(delivery.kind)),
            category: delivery.category().into(),
            failed_at: delivery.failed_at,
            result: delivery.last_result.as_ref().map(serde_str),
            http_status: delivery.last_http_status,
            failure_reason: delivery.failure_reason.as_ref().map(serde_str),
            attempt_count: delivery.attempt_count,
        }
    }
}

impl From<&WebhookFailedPage> for WebhookFailedDeliveriesResponse {
    fn from(page: &WebhookFailedPage) -> Self {
        Self {
            items: page.items.iter().map(Into::into).collect(),
            next_cursor: page.next_cursor.clone(),
        }
    }
}

impl WebhookTestResponse {
    /// 送信した場合のみ応答にできる (環境変数の送信先の未設定・設定不備は呼び出し側が 409 にする)。
    pub fn from_outcome(outcome: &WebhookTestOutcome) -> Option<Self> {
        match outcome {
            WebhookTestOutcome::Sent {
                event_id,
                delivered,
                result,
                http_status,
                attempted_at,
            } => Some(Self {
                event_id: event_id.clone(),
                delivered: *delivered,
                result: serde_str(result),
                http_status: *http_status,
                attempted_at: *attempted_at,
            }),
            WebhookTestOutcome::NotConfigured | WebhookTestOutcome::Misconfigured => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_ids_round_trip_and_reject_garbage() {
        assert_eq!(
            endpoint_id_to_wire(WebhookEndpointId::ENVIRONMENT),
            "environment"
        );
        assert_eq!(
            parse_endpoint_id("environment"),
            Some(WebhookEndpointId::ENVIRONMENT)
        );
        assert_eq!(parse_endpoint_id("42"), Some(WebhookEndpointId::new(42)));
        for bad in ["", "0", "-1", "abc", "1.5", "environment2"] {
            assert_eq!(parse_endpoint_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn requests_never_debug_print_a_secret() {
        let create: CreateWebhookEndpointRequest = serde_json::from_value(serde_json::json!({
            "url": "https://hooks.example.net/x",
            "secret": "very-secret-value-0123456789",
            "categories": ["outage", "auto_stop"],
        }))
        .unwrap();
        assert!(!format!("{create:?}").contains("very-secret"));
        let update: UpdateWebhookEndpointRequest = serde_json::from_value(serde_json::json!({
            "revision": 3,
            "secret": "very-secret-value-0123456789",
        }))
        .unwrap();
        assert!(!format!("{update:?}").contains("very-secret"));
        assert!(update.url.is_none() && update.categories.is_none());
    }

    #[test]
    fn category_identifiers_in_the_api_are_the_documented_ones() {
        let all = [
            WebhookCategoryId::Outage,
            WebhookCategoryId::Recovery,
            WebhookCategoryId::AutoStop,
            WebhookCategoryId::Override,
        ];
        let json = serde_json::to_value(all).unwrap();
        assert_eq!(
            json,
            serde_json::json!(["outage", "recovery", "auto_stop", "override"])
        );
        for id in all {
            let category: WebhookCategory = id.into();
            assert_eq!(WebhookCategoryId::from(category), id);
        }
        assert!(
            serde_json::from_value::<WebhookCategoryId>(serde_json::json!("webhook_test")).is_err()
        );
    }
}
