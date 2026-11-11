//! 登録ゲート Webhook の送信先管理 (E15x) のドメイン。
//!
//! 管理者が登録する複数の送信先 (URL・書き込み専用 secret・送る通知カテゴリ) の型と、I/O のない
//! 純粋ロジック (カテゴリ ⇔ `GateEventKind` の対応・URL の部分マスク・失敗一覧の cursor・secret 型)
//! だけを置く。
//!
//! 重要な不変条件:
//! - `WebhookSecret` は `Debug`/`Display` に値を出さず、drop 時にゼロ埋めする。
//! - URL の全体・path・query・userinfo は管理者 API に返さない (`mask_webhook_url` の結果だけ)。
//! - 送信先の変更・配送失敗・監査は通知イベント (= Webhook 配送対象) を生まない。配送対象は
//!   `GateEventKind` だけで、送信先管理の型からは `GateEventKind` を作る経路が存在しない。

use crate::entity::{
    AuthAccountId, GateEventKind, RegistrationGateEventId, WebhookConfigIssue,
    WebhookDeliveryCounts, WebhookDeliverySummary, WebhookFailureReason, WebhookResultClass,
};
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Formatter};
use time::OffsetDateTime;
use zeroize::Zeroize;

/// 登録できる送信先の上限件数 (超過は `EndpointLimitReached`)。
pub const MAX_WEBHOOK_ENDPOINTS: i64 = 10;
/// URL の長さ上限 (文字数)。
pub const MAX_WEBHOOK_URL_LENGTH: usize = 2048;
/// 署名 secret の最小長 (バイト。E15w と同じ)。
pub const MIN_WEBHOOK_SECRET_BYTES: usize = 16;
/// 署名 secret の最大長 (バイト)。
pub const MAX_WEBHOOK_SECRET_BYTES: usize = 256;
/// マスク表示で path の末尾を出す最小トークン長 (これ未満は末尾も伏せる)。
pub const MASK_MIN_TOKEN_LENGTH: usize = 12;
/// マスク表示で見せる末尾の文字数。
pub const MASK_VISIBLE_TAIL: usize = 4;
const MASK: &str = "••••";

// ---------------------------------------------------------------------------------------------
// 送信先 ID
// ---------------------------------------------------------------------------------------------

/// 送信先の ID。`0` は環境変数で設定された読み取り専用の送信先 (E15w の移行前の 1 件) を表す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WebhookEndpointId(i64);

impl WebhookEndpointId {
    /// 環境変数の送信先 (read-only)。
    pub const ENVIRONMENT: Self = Self(0);

    pub fn new(value: i64) -> Self {
        Self(value)
    }

    pub fn generate() -> Self {
        Self(crate::generate_id())
    }

    pub fn as_i64(&self) -> i64 {
        self.0
    }

    pub fn is_environment(&self) -> bool {
        self.0 == 0
    }
}

// ---------------------------------------------------------------------------------------------
// 通知カテゴリ ⇔ GateEventKind
// ---------------------------------------------------------------------------------------------

/// UI の「送る通知」の 4 カテゴリ。識別子は API で固定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookCategory {
    /// 障害 (`outage_started` / `protection_degraded`)。
    Outage,
    /// 復旧 (`outage_recovered` / `enforcement_restored`)。
    Recovery,
    /// 自動停止 (`protection_lost_stopped` / `enforcement_lost_stopped`)。
    AutoStop,
    /// 強制開放 (`override_started` / `override_revoked`)。
    Override,
}

impl WebhookCategory {
    pub const ALL: [WebhookCategory; 4] = [
        WebhookCategory::Outage,
        WebhookCategory::Recovery,
        WebhookCategory::AutoStop,
        WebhookCategory::Override,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            WebhookCategory::Outage => "outage",
            WebhookCategory::Recovery => "recovery",
            WebhookCategory::AutoStop => "auto_stop",
            WebhookCategory::Override => "override",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }

    /// このカテゴリに含まれる通知イベント種別。
    pub fn kinds(&self) -> Vec<GateEventKind> {
        ALL_GATE_EVENT_KINDS
            .into_iter()
            .filter(|kind| category_of(*kind) == *self)
            .collect()
    }
}

/// 通知イベント種別の全列挙。`category_of` の網羅 match と下のテストで、新種別の追加時に
/// 分類と列挙の更新が必ず迫られる。
pub const ALL_GATE_EVENT_KINDS: [GateEventKind; 8] = [
    GateEventKind::OutageStarted,
    GateEventKind::OutageRecovered,
    GateEventKind::ProtectionDegraded,
    GateEventKind::ProtectionLostStopped,
    GateEventKind::EnforcementLostStopped,
    GateEventKind::EnforcementRestored,
    GateEventKind::OverrideStarted,
    GateEventKind::OverrideRevoked,
];

/// 通知イベント種別が属するカテゴリ (ちょうど 1 つ)。**ワイルドカード禁止の網羅 match**: 新しい
/// `GateEventKind` を追加するとここがコンパイルエラーになり、分類を決めさせられる。
pub fn category_of(kind: GateEventKind) -> WebhookCategory {
    match kind {
        GateEventKind::OutageStarted => WebhookCategory::Outage,
        GateEventKind::ProtectionDegraded => WebhookCategory::Outage,
        GateEventKind::OutageRecovered => WebhookCategory::Recovery,
        GateEventKind::EnforcementRestored => WebhookCategory::Recovery,
        GateEventKind::ProtectionLostStopped => WebhookCategory::AutoStop,
        GateEventKind::EnforcementLostStopped => WebhookCategory::AutoStop,
        GateEventKind::OverrideStarted => WebhookCategory::Override,
        GateEventKind::OverrideRevoked => WebhookCategory::Override,
    }
}

/// 送る通知カテゴリの選択 (1 つ以上、重複なし、`WebhookCategory::ALL` の順)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookCategories(Vec<WebhookCategory>);

impl WebhookCategories {
    /// 空の選択は拒否 (`None`)。重複は除く。
    pub fn new(categories: impl IntoIterator<Item = WebhookCategory>) -> Option<Self> {
        let given: Vec<WebhookCategory> = categories.into_iter().collect();
        let ordered: Vec<WebhookCategory> = WebhookCategory::ALL
            .into_iter()
            .filter(|category| given.contains(category))
            .collect();
        (!ordered.is_empty()).then_some(Self(ordered))
    }

    pub fn all() -> Self {
        Self(WebhookCategory::ALL.to_vec())
    }

    pub fn as_slice(&self) -> &[WebhookCategory] {
        &self.0
    }

    pub fn contains(&self, category: WebhookCategory) -> bool {
        self.0.contains(&category)
    }

    pub fn subscribes_to(&self, kind: GateEventKind) -> bool {
        self.contains(category_of(kind))
    }

    /// 購読している通知イベント種別の全体。
    pub fn kinds(&self) -> Vec<GateEventKind> {
        ALL_GATE_EVENT_KINDS
            .into_iter()
            .filter(|kind| self.subscribes_to(*kind))
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------
// secret
// ---------------------------------------------------------------------------------------------

/// secret の入力が拒否された理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebhookSecretRejection {
    TooShort,
    TooLong,
}

/// 署名 secret (書き込み専用)。`Debug`/`Display` に値を出さず、drop 時にゼロ埋めする。
#[derive(Clone)]
pub struct WebhookSecret(Vec<u8>);

impl WebhookSecret {
    /// 前後の空白を除いた長さが 16〜256 バイトであること。
    pub fn parse(raw: &str) -> Result<Self, WebhookSecretRejection> {
        Self::from_bytes(raw.trim().as_bytes().to_vec())
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, WebhookSecretRejection> {
        if bytes.len() < MIN_WEBHOOK_SECRET_BYTES {
            let mut bytes = bytes;
            bytes.zeroize();
            return Err(WebhookSecretRejection::TooShort);
        }
        if bytes.len() > MAX_WEBHOOK_SECRET_BYTES {
            let mut bytes = bytes;
            bytes.zeroize();
            return Err(WebhookSecretRejection::TooLong);
        }
        Ok(Self(bytes))
    }

    /// 上限なしの構築。サーバー設定 (環境変数の secret。E15w は上限なし) と、自分で暗号化した
    /// 暗号文の復号結果にだけ使う。API の入力には `parse` を使う。
    pub fn from_trusted_bytes(bytes: Vec<u8>) -> Result<Self, WebhookSecretRejection> {
        if bytes.len() < MIN_WEBHOOK_SECRET_BYTES {
            let mut bytes = bytes;
            bytes.zeroize();
            return Err(WebhookSecretRejection::TooShort);
        }
        Ok(Self(bytes))
    }

    /// 署名・暗号化の入力としてだけ使う。ログ・応答に出さない。
    pub fn expose_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Debug for WebhookSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookSecret(<redacted>)")
    }
}

impl Drop for WebhookSecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// API で受け取った secret の生の入力 (書き込み専用)。検査前の文字列で、`Debug` に値を出さず、
/// drop 時にゼロ埋めする。リクエストのデシリアライズ用に透過的に `Deserialize` できる。
#[derive(Deserialize)]
#[serde(transparent)]
pub struct WriteOnlySecret(String);

impl WriteOnlySecret {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// 長さを検査して `WebhookSecret` にする。
    pub fn parse(&self) -> Result<WebhookSecret, WebhookSecretRejection> {
        WebhookSecret::parse(&self.0)
    }
}

impl Debug for WriteOnlySecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("WriteOnlySecret(<redacted>)")
    }
}

impl Drop for WriteOnlySecret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// 暗号化済みの secret。暗号文は鍵識別子を持つ (将来の鍵ローテーションに備える)。
#[derive(Clone, PartialEq, Eq)]
pub struct SealedWebhookSecret {
    pub key_id: String,
    pub ciphertext: Vec<u8>,
}

impl Debug for SealedWebhookSecret {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedWebhookSecret")
            .field("key_id", &self.key_id)
            .field("ciphertext", &"<redacted>")
            .finish()
    }
}

/// 送信先 (検査済み URL + 復号済み secret)。送信のたびに組み立て、保持しない。
#[derive(Clone)]
pub struct WebhookDestination {
    pub url: String,
    pub secret: WebhookSecret,
}

impl Debug for WebhookDestination {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookDestination(<redacted>)")
    }
}

// ---------------------------------------------------------------------------------------------
// 拒否理由 (登録・更新)
// ---------------------------------------------------------------------------------------------

/// 送信先の登録・更新が拒否された理由。内部ネットワークの探索に使えるほど詳細な情報
/// (どの内部アドレスに解決したか等) は含まない固定語彙。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEndpointRejection {
    InvalidUrl,
    UnsupportedScheme,
    CredentialsInUrl,
    FragmentInUrl,
    MissingHost,
    UrlTooLong,
    /// localhost 系・内部アドレスなど (リテラル、または名前解決結果のいずれか)。
    ForbiddenDestination,
    /// 名前解決できない (または解決結果が空)。
    HostUnresolvable,
    SecretTooShort,
    SecretTooLong,
    /// secret が必要なのに指定されていない。
    SecretRequired,
    /// 通知カテゴリが空。
    NoCategories,
    /// 正規化後の URL が既存の送信先と同じ。
    DuplicateUrl,
    EndpointLimitReached,
    /// 暗号鍵がサーバー設定にない (secret を平文で保存しないので登録・secret 更新を拒否する)。
    EncryptionKeyNotConfigured,
    /// 環境変数の送信先は API から編集・削除できない。
    EnvironmentEndpointReadOnly,
    /// 更新する項目がない。
    NothingToUpdate,
}

impl WebhookEndpointRejection {
    pub fn code(&self) -> &'static str {
        match self {
            WebhookEndpointRejection::InvalidUrl => "invalid_url",
            WebhookEndpointRejection::UnsupportedScheme => "unsupported_scheme",
            WebhookEndpointRejection::CredentialsInUrl => "credentials_in_url",
            WebhookEndpointRejection::FragmentInUrl => "fragment_in_url",
            WebhookEndpointRejection::MissingHost => "missing_host",
            WebhookEndpointRejection::UrlTooLong => "url_too_long",
            WebhookEndpointRejection::ForbiddenDestination => "forbidden_destination",
            WebhookEndpointRejection::HostUnresolvable => "host_unresolvable",
            WebhookEndpointRejection::SecretTooShort => "secret_too_short",
            WebhookEndpointRejection::SecretTooLong => "secret_too_long",
            WebhookEndpointRejection::SecretRequired => "secret_required",
            WebhookEndpointRejection::NoCategories => "no_categories",
            WebhookEndpointRejection::DuplicateUrl => "duplicate_url",
            WebhookEndpointRejection::EndpointLimitReached => "endpoint_limit_reached",
            WebhookEndpointRejection::EncryptionKeyNotConfigured => "encryption_key_not_configured",
            WebhookEndpointRejection::EnvironmentEndpointReadOnly => {
                "environment_endpoint_read_only"
            }
            WebhookEndpointRejection::NothingToUpdate => "nothing_to_update",
        }
    }
}

impl std::fmt::Display for WebhookEndpointRejection {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.code())
    }
}

impl From<WebhookSecretRejection> for WebhookEndpointRejection {
    fn from(rejection: WebhookSecretRejection) -> Self {
        match rejection {
            WebhookSecretRejection::TooShort => WebhookEndpointRejection::SecretTooShort,
            WebhookSecretRejection::TooLong => WebhookEndpointRejection::SecretTooLong,
        }
    }
}

impl From<WebhookConfigIssue> for WebhookEndpointRejection {
    fn from(issue: WebhookConfigIssue) -> Self {
        match issue {
            WebhookConfigIssue::InvalidUrl => WebhookEndpointRejection::InvalidUrl,
            WebhookConfigIssue::UnsupportedScheme => WebhookEndpointRejection::UnsupportedScheme,
            WebhookConfigIssue::CredentialsInUrl => WebhookEndpointRejection::CredentialsInUrl,
            WebhookConfigIssue::MissingHost => WebhookEndpointRejection::MissingHost,
            WebhookConfigIssue::ForbiddenDestination => {
                WebhookEndpointRejection::ForbiddenDestination
            }
            WebhookConfigIssue::MissingSecret
            | WebhookConfigIssue::SecretTooShort
            | WebhookConfigIssue::SecretUnreadable
            | WebhookConfigIssue::AmbiguousSecretSource
            | WebhookConfigIssue::MissingUrl => WebhookEndpointRejection::InvalidUrl,
        }
    }
}

/// 登録時の宛先検査を通った URL (正規化済み)。
#[derive(Clone, PartialEq, Eq)]
pub struct ValidatedWebhookUrl(String);

impl ValidatedWebhookUrl {
    /// 宛先検査の実装 (driver) だけが作る想定。
    pub fn new_checked(normalized: String) -> Self {
        Self(normalized)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl Debug for ValidatedWebhookUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("ValidatedWebhookUrl(<redacted>)")
    }
}

// ---------------------------------------------------------------------------------------------
// URL の部分マスク
// ---------------------------------------------------------------------------------------------

/// 一覧・取得に出す URL の部分マスク形。host (ポートがあれば含む) と、path があるときは
/// 伏せ字 + 末尾数文字。**全 URL・path・query・userinfo は返さない**。
///
/// - path も query もない: `host`
/// - path の最後のトークンが 12 文字以上: `host/••••/<末尾 4 文字>`
/// - それ以外 (短いトークン・query のみ): `host/••••` (末尾を出さない)
pub fn mask_webhook_url(raw: &str) -> String {
    let Ok(url) = url::Url::parse(raw) else {
        return MASK.to_string();
    };
    let Some(host) = url.host_str() else {
        return MASK.to_string();
    };
    let authority = match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    let last_token = url
        .path_segments()
        .and_then(|segments| segments.rev().find(|segment| !segment.is_empty()));
    match last_token {
        Some(token) if token.chars().count() >= MASK_MIN_TOKEN_LENGTH => {
            let tail: Vec<char> = token.chars().rev().take(MASK_VISIBLE_TAIL).collect();
            let tail: String = tail.into_iter().rev().collect();
            format!("{authority}/{MASK}/{tail}")
        }
        Some(_) => format!("{authority}/{MASK}"),
        None if url.query().is_some() => format!("{authority}/{MASK}"),
        None => authority,
    }
}

// ---------------------------------------------------------------------------------------------
// 送信先の永続表現と管理者向けビュー
// ---------------------------------------------------------------------------------------------

/// 送信先の出所。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEndpointSource {
    /// 管理者が API で登録した。
    Api,
    /// 環境変数の 1 件を初期送信先として取り込んだ (以後は DB が正本)。
    EnvironmentImport,
    /// 暗号鍵が未設定のため環境変数のまま配送している (read-only。API から編集・削除できない)。
    Environment,
}

/// DB の送信先 1 件 (内部用。URL 全体と暗号文を含むので API に出さない)。
#[derive(Clone, PartialEq, Eq)]
pub struct StoredWebhookEndpoint {
    pub id: WebhookEndpointId,
    pub url: String,
    pub sealed_secret: SealedWebhookSecret,
    pub categories: WebhookCategories,
    pub revision: i64,
    /// この ID より後の通知イベントだけを配送する (作成前のイベントは遡及しない)。
    pub baseline_event_id: i64,
    pub source: WebhookEndpointSource,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl Debug for StoredWebhookEndpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredWebhookEndpoint")
            .field("id", &self.id)
            .field("revision", &self.revision)
            .field("categories", &self.categories)
            .finish_non_exhaustive()
    }
}

/// 新規作成する送信先。
#[derive(Clone, PartialEq, Eq)]
pub struct NewWebhookEndpoint {
    pub id: WebhookEndpointId,
    pub url: ValidatedWebhookUrl,
    pub sealed_secret: SealedWebhookSecret,
    pub categories: WebhookCategories,
    pub source: WebhookEndpointSource,
}

impl Debug for NewWebhookEndpoint {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NewWebhookEndpoint")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// 更新する項目 (None は「変更しない」)。
#[derive(Clone, PartialEq, Eq, Default)]
pub struct WebhookEndpointPatch {
    pub url: Option<ValidatedWebhookUrl>,
    pub sealed_secret: Option<SealedWebhookSecret>,
    pub categories: Option<WebhookCategories>,
}

impl Debug for WebhookEndpointPatch {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebhookEndpointPatch")
            .field("url", &self.url.is_some())
            .field("secret", &self.sealed_secret.is_some())
            .field("categories", &self.categories)
            .finish()
    }
}

/// 書き込み操作の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebhookEndpointWrite {
    Done(Box<StoredWebhookEndpoint>),
    DuplicateUrl,
    LimitReached,
    RevisionConflict,
    NotFound,
}

/// 送信先の状態チップ。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEndpointHealth {
    Healthy,
    Failing,
}

/// `failing` の理由 (複数ありうる)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookFailingReason {
    /// 未解決の failed 配送がある。
    FailedDeliveries,
    /// 保存済み secret を復号できない (鍵の喪失・誤り・変更)。secret の再入力で復旧する。
    SecretUndecryptable,
    /// 環境変数の送信先の設定不備。
    ConfigInvalid,
}

/// 状態の定義: `failing` = 未解決の failed 配送がある、または復号不可・設定不備などの恒久的な
/// 理由がある。failed が再試行で解消されれば `healthy` に戻る。
pub fn endpoint_health(reasons: &[WebhookFailingReason]) -> WebhookEndpointHealth {
    if reasons.is_empty() {
        WebhookEndpointHealth::Healthy
    } else {
        WebhookEndpointHealth::Failing
    }
}

/// 管理者に見せる送信先。URL は部分マスクのみ、secret は設定済みかの真偽値のみ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookEndpointView {
    pub id: WebhookEndpointId,
    pub source: WebhookEndpointSource,
    pub masked_url: String,
    pub categories: WebhookCategories,
    /// 環境変数の送信先には revision がない。
    pub revision: Option<i64>,
    pub health: WebhookEndpointHealth,
    pub failing_reasons: Vec<WebhookFailingReason>,
    pub failed_count: i64,
    pub summary: WebhookDeliverySummary,
    pub secret_configured: bool,
    /// 環境変数の送信先の設定不備 (分類のみ)。
    pub config_issue: Option<WebhookConfigIssue>,
    pub created_at: Option<OffsetDateTime>,
    pub updated_at: Option<OffsetDateTime>,
}

impl WebhookEndpointView {
    pub fn editable(&self) -> bool {
        self.source != WebhookEndpointSource::Environment
    }
}

/// 送信先ごとの配送集計。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebhookEndpointStats {
    pub counts: WebhookDeliveryCounts,
    pub last_success_at: Option<OffsetDateTime>,
    pub last_failure_at: Option<OffsetDateTime>,
}

impl From<WebhookEndpointStats> for WebhookDeliverySummary {
    fn from(stats: WebhookEndpointStats) -> Self {
        WebhookDeliverySummary {
            counts: stats.counts,
            last_success_at: stats.last_success_at,
            last_failure_at: stats.last_failure_at,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// 配送失敗の一覧 (cursor ページング)
// ---------------------------------------------------------------------------------------------

/// 配送失敗 1 件 (本文・URL・secret を含まない)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookFailedDelivery {
    pub endpoint_id: WebhookEndpointId,
    pub event_id: RegistrationGateEventId,
    pub kind: GateEventKind,
    pub failed_at: OffsetDateTime,
    pub last_result: Option<WebhookResultClass>,
    pub last_http_status: Option<u16>,
    pub failure_reason: Option<WebhookFailureReason>,
    pub attempt_count: u32,
}

impl WebhookFailedDelivery {
    pub fn category(&self) -> WebhookCategory {
        category_of(self.kind)
    }

    pub fn cursor(&self) -> WebhookFailureCursor {
        WebhookFailureCursor {
            failed_at: self.failed_at,
            endpoint_id: self.endpoint_id.as_i64(),
            event_id: *self.event_id.as_ref(),
        }
    }
}

/// failed 一覧の位置。並び順は (failed_at, endpoint_id, event_id) の降順 (決定的)。
/// `before` に渡すとこれより後ろ (古い側) を返す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebhookFailureCursor {
    pub failed_at: OffsetDateTime,
    pub endpoint_id: i64,
    pub event_id: i64,
}

impl WebhookFailureCursor {
    /// 不透明な文字列 (`<unix マイクロ秒>.<送信先 ID>.<event ID>`)。
    pub fn encode(&self) -> String {
        let micros = self.failed_at.unix_timestamp_nanos() / 1000;
        format!("{micros}.{}.{}", self.endpoint_id, self.event_id)
    }

    pub fn decode(raw: &str) -> Option<Self> {
        let mut parts = raw.split('.');
        let micros: i128 = parts.next()?.parse().ok()?;
        let endpoint_id: i64 = parts.next()?.parse().ok()?;
        let event_id: i64 = parts.next()?.parse().ok()?;
        if parts.next().is_some() || endpoint_id < 0 || event_id < 0 {
            return None;
        }
        let failed_at =
            OffsetDateTime::from_unix_timestamp_nanos(micros.checked_mul(1000)?).ok()?;
        Some(Self {
            failed_at,
            endpoint_id,
            event_id,
        })
    }
}

/// failed 一覧の 1 ページ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookFailedPage {
    pub items: Vec<WebhookFailedDelivery>,
    pub next_cursor: Option<String>,
}

pub const DEFAULT_FAILED_PAGE_LIMIT: i64 = 50;
pub const MAX_FAILED_PAGE_LIMIT: i64 = 100;

/// 取得件数 (+1 件先読みして次ページの有無を判定する) からページを組み立てる。
pub fn build_failed_page(mut rows: Vec<WebhookFailedDelivery>, limit: i64) -> WebhookFailedPage {
    let limit = limit.clamp(1, MAX_FAILED_PAGE_LIMIT) as usize;
    let has_more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = if has_more {
        rows.last().map(|last| last.cursor().encode())
    } else {
        None
    };
    WebhookFailedPage {
        items: rows,
        next_cursor,
    }
}

// ---------------------------------------------------------------------------------------------
// 監査
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookAuditOperation {
    Create,
    Update,
    Delete,
    Test,
    Retry,
    /// 環境変数の送信先の初期取り込み (起動時。操作者なし)。
    Import,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebhookAuditOutcome {
    Succeeded,
    Rejected,
    RevisionConflict,
    NotFound,
    /// テスト送信で配送できなかった (分類は `result_class`)。
    DeliveryFailed,
}

/// 監査 1 件。操作者・対象送信先 ID・操作種別・結果・revision だけを残す。
/// URL・secret・通知本文は残さない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookAuditEntry {
    pub actor: Option<AuthAccountId>,
    pub endpoint_id: Option<WebhookEndpointId>,
    pub operation: WebhookAuditOperation,
    pub outcome: WebhookAuditOutcome,
    pub revision: Option<i64>,
    /// 拒否理由 (固定語彙) またはテスト送信の結果分類など。
    pub detail: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_gate_event_kind_belongs_to_exactly_one_category() {
        // この match はワイルドカードなし: 新しい GateEventKind はここでもコンパイルエラーになる。
        for kind in ALL_GATE_EVENT_KINDS {
            match kind {
                GateEventKind::OutageStarted
                | GateEventKind::OutageRecovered
                | GateEventKind::ProtectionDegraded
                | GateEventKind::ProtectionLostStopped
                | GateEventKind::EnforcementLostStopped
                | GateEventKind::EnforcementRestored
                | GateEventKind::OverrideStarted
                | GateEventKind::OverrideRevoked => {}
            }
            let owners = WebhookCategory::ALL
                .into_iter()
                .filter(|category| category.kinds().contains(&kind))
                .count();
            assert_eq!(owners, 1, "{kind:?} must belong to exactly one category");
        }
        let total: usize = WebhookCategory::ALL
            .into_iter()
            .map(|category| category.kinds().len())
            .sum();
        assert_eq!(total, ALL_GATE_EVENT_KINDS.len());
    }

    #[test]
    fn category_table_matches_the_documented_mapping() {
        use GateEventKind::*;
        let expected = [
            (OutageStarted, WebhookCategory::Outage),
            (ProtectionDegraded, WebhookCategory::Outage),
            (OutageRecovered, WebhookCategory::Recovery),
            (EnforcementRestored, WebhookCategory::Recovery),
            (ProtectionLostStopped, WebhookCategory::AutoStop),
            (EnforcementLostStopped, WebhookCategory::AutoStop),
            (OverrideStarted, WebhookCategory::Override),
            (OverrideRevoked, WebhookCategory::Override),
        ];
        for (kind, category) in expected {
            assert_eq!(category_of(kind), category);
        }
    }

    #[test]
    fn category_identifiers_are_fixed_and_round_trip() {
        let identifiers: Vec<&str> = WebhookCategory::ALL.iter().map(|c| c.as_str()).collect();
        assert_eq!(identifiers, ["outage", "recovery", "auto_stop", "override"]);
        for category in WebhookCategory::ALL {
            assert_eq!(WebhookCategory::parse(category.as_str()), Some(category));
            assert_eq!(
                serde_json::to_value(category).unwrap(),
                serde_json::Value::String(category.as_str().to_string())
            );
        }
        assert_eq!(WebhookCategory::parse("webhook_test"), None);
    }

    #[test]
    fn categories_must_be_non_empty_deduplicated_and_ordered() {
        assert!(WebhookCategories::new([]).is_none());
        let categories = WebhookCategories::new([
            WebhookCategory::Override,
            WebhookCategory::Outage,
            WebhookCategory::Outage,
        ])
        .unwrap();
        assert_eq!(
            categories.as_slice(),
            [WebhookCategory::Outage, WebhookCategory::Override]
        );
        assert!(categories.subscribes_to(GateEventKind::OutageStarted));
        assert!(!categories.subscribes_to(GateEventKind::OutageRecovered));
        assert_eq!(WebhookCategories::all().kinds().len(), 8);
    }

    #[test]
    fn secret_debug_never_prints_the_value() {
        let secret = WebhookSecret::parse("super-secret-value-0123456789").unwrap();
        let rendered = format!("{secret:?}");
        assert!(!rendered.contains("super-secret"));
        let destination = WebhookDestination {
            url: "https://hooks.example.net/very/secret/path-token-123456".to_string(),
            secret,
        };
        let rendered = format!("{destination:?}");
        assert!(!rendered.contains("super-secret"));
        assert!(!rendered.contains("path-token"));
    }

    #[test]
    fn secret_length_limits() {
        assert_eq!(
            WebhookSecret::parse("short").unwrap_err(),
            WebhookSecretRejection::TooShort
        );
        assert!(WebhookSecret::parse(&"a".repeat(16)).is_ok());
        assert!(WebhookSecret::parse(&"a".repeat(256)).is_ok());
        assert_eq!(
            WebhookSecret::parse(&"a".repeat(257)).unwrap_err(),
            WebhookSecretRejection::TooLong
        );
        // 前後の空白は数えない。
        assert_eq!(
            WebhookSecret::parse(&format!("  {}  ", "a".repeat(15))).unwrap_err(),
            WebhookSecretRejection::TooShort
        );
    }

    #[test]
    fn sealed_secret_and_stored_endpoint_debug_hide_material() {
        let sealed = SealedWebhookSecret {
            key_id: "kf-0123".to_string(),
            ciphertext: vec![0xde, 0xad, 0xbe, 0xef],
        };
        assert!(!format!("{sealed:?}").contains("222"));
        let stored = StoredWebhookEndpoint {
            id: WebhookEndpointId::new(7),
            url: "https://hooks.example.net/secret-token-value".to_string(),
            sealed_secret: sealed,
            categories: WebhookCategories::all(),
            revision: 1,
            baseline_event_id: 0,
            source: WebhookEndpointSource::Api,
            created_at: OffsetDateTime::UNIX_EPOCH,
            updated_at: OffsetDateTime::UNIX_EPOCH,
        };
        assert!(!format!("{stored:?}").contains("secret-token-value"));
    }

    #[test]
    fn mask_shows_host_and_only_a_short_tail_of_long_tokens() {
        assert_eq!(
            mask_webhook_url("https://hooks.example.net/services/T000/B000/abcdefgh12a8f2"),
            "hooks.example.net/••••/a8f2"
        );
        assert_eq!(
            mask_webhook_url("https://hooks.example.net/hook/abcdefghijkl"),
            "hooks.example.net/••••/ijkl"
        );
    }

    #[test]
    fn mask_hides_the_tail_of_short_tokens() {
        assert_eq!(
            mask_webhook_url("https://hooks.example.net/abcdefghijk"),
            "hooks.example.net/••••"
        );
        assert_eq!(
            mask_webhook_url("https://hooks.example.net/in/short"),
            "hooks.example.net/••••"
        );
    }

    #[test]
    fn mask_never_reveals_query_userinfo_fragment_or_full_path() {
        let masked = mask_webhook_url(
            "https://user:pw@hooks.example.net:8443/a/b/token-0123456789abcdef?key=querysecret#frag",
        );
        assert_eq!(masked, "hooks.example.net:8443/••••/cdef");
        for leaked in ["user", "pw", "querysecret", "frag", "token-0123", "/a/b"] {
            assert!(!masked.contains(leaked), "{masked} leaks {leaked}");
        }
        assert_eq!(
            mask_webhook_url("https://hooks.example.net/?key=querysecret0123456789"),
            "hooks.example.net/••••"
        );
        assert_eq!(
            mask_webhook_url("https://hooks.example.net/"),
            "hooks.example.net"
        );
        assert_eq!(
            mask_webhook_url("https://hooks.example.net"),
            "hooks.example.net"
        );
    }

    #[test]
    fn mask_handles_ipv6_literals_and_trailing_slash() {
        assert_eq!(
            mask_webhook_url("https://[2001:db8::1]:8443/abcdefghijklmnop/"),
            "[2001:db8::1]:8443/••••/mnop"
        );
        assert_eq!(mask_webhook_url("not a url"), "••••");
    }

    #[test]
    fn failure_cursor_round_trips_and_rejects_garbage() {
        let cursor = WebhookFailureCursor {
            failed_at: OffsetDateTime::from_unix_timestamp_nanos(1_700_000_000_123_456_000)
                .unwrap(),
            endpoint_id: 42,
            event_id: 7,
        };
        assert_eq!(WebhookFailureCursor::decode(&cursor.encode()), Some(cursor));
        for bad in ["", "1.2", "a.b.c", "1.2.3.4", "1.-2.3", "1.2.-3", "1e3.2.3"] {
            assert_eq!(WebhookFailureCursor::decode(bad), None, "{bad}");
        }
    }

    fn failed(endpoint: i64, event: i64, micros: i64) -> WebhookFailedDelivery {
        WebhookFailedDelivery {
            endpoint_id: WebhookEndpointId::new(endpoint),
            event_id: RegistrationGateEventId::new(event),
            kind: GateEventKind::OutageStarted,
            failed_at: OffsetDateTime::from_unix_timestamp_nanos(i128::from(micros) * 1000)
                .unwrap(),
            last_result: Some(WebhookResultClass::HttpServerError),
            last_http_status: Some(500),
            failure_reason: Some(WebhookFailureReason::RetriesExhausted),
            attempt_count: 8,
        }
    }

    #[test]
    fn failed_page_has_a_cursor_only_when_more_rows_remain() {
        let rows = vec![failed(1, 3, 30), failed(1, 2, 20), failed(1, 1, 10)];
        let page = build_failed_page(rows.clone(), 2);
        assert_eq!(page.items.len(), 2);
        assert_eq!(
            page.next_cursor.as_deref(),
            Some(page.items[1].cursor().encode().as_str())
        );
        let last = build_failed_page(rows, 3);
        assert_eq!(last.items.len(), 3);
        assert!(last.next_cursor.is_none());
    }

    #[test]
    fn health_is_failing_iff_there_is_a_reason() {
        assert_eq!(endpoint_health(&[]), WebhookEndpointHealth::Healthy);
        assert_eq!(
            endpoint_health(&[WebhookFailingReason::FailedDeliveries]),
            WebhookEndpointHealth::Failing
        );
    }

    #[test]
    fn rejection_codes_are_unique() {
        use WebhookEndpointRejection::*;
        let all = [
            InvalidUrl,
            UnsupportedScheme,
            CredentialsInUrl,
            FragmentInUrl,
            MissingHost,
            UrlTooLong,
            ForbiddenDestination,
            HostUnresolvable,
            SecretTooShort,
            SecretTooLong,
            SecretRequired,
            NoCategories,
            DuplicateUrl,
            EndpointLimitReached,
            EncryptionKeyNotConfigured,
            EnvironmentEndpointReadOnly,
            NothingToUpdate,
        ];
        let codes: std::collections::HashSet<_> = all.iter().map(|r| r.code()).collect();
        assert_eq!(codes.len(), all.len());
    }
}
