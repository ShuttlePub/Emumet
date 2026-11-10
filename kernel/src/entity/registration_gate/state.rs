use crate::entity::AuthAccountId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// 登録を守る対策 (q01: メール認証 OR BOT 対策)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protection {
    Email,
    Bot,
}

impl Protection {
    pub const ALL: [Protection; 2] = [Protection::Email, Protection::Bot];
}

/// 検証 API の観測結果 (q05)。`Unknown` は「まだ実応答を確認できていない」ことを表し、有効扱いしない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Healthy,
    Unhealthy,
    Unknown,
}

/// 対策ごとの観測入力: 設定上の有無 + 検証 API の観測結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtectionObservation {
    pub configured: bool,
    pub probe: ProbeStatus,
}

impl ProtectionObservation {
    pub const UNCONFIGURED: Self = Self {
        configured: false,
        probe: ProbeStatus::Unknown,
    };

    pub fn new(configured: bool, probe: ProbeStatus) -> Self {
        Self { configured, probe }
    }

    /// `effective(P)` = 設定上有効 かつ 観測が Healthy (q05)。
    pub fn is_effective(&self) -> bool {
        self.configured && self.probe == ProbeStatus::Healthy
    }
}

/// 観測入力。テスト fixture と内部 use case からのみ供給する (公開 HTTP の書込口は持たない)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RegistrationObservation {
    /// Trusted deployment attestation (not provider health). Missing old payloads predate enforcement.
    #[serde(default = "verified_by_legacy_default")]
    pub enforcement_verified: bool,
    pub email: ProtectionObservation,
    pub bot: ProtectionObservation,
}

impl RegistrationObservation {
    pub const UNCONFIGURED: Self = Self {
        enforcement_verified: true,
        email: ProtectionObservation::UNCONFIGURED,
        bot: ProtectionObservation::UNCONFIGURED,
    };

    pub fn new(email: ProtectionObservation, bot: ProtectionObservation) -> Self {
        Self {
            email,
            bot,
            enforcement_verified: true,
        }
    }

    pub fn get(&self, protection: Protection) -> &ProtectionObservation {
        match protection {
            Protection::Email => &self.email,
            Protection::Bot => &self.bot,
        }
    }

    pub fn configured_count(&self) -> usize {
        Protection::ALL
            .iter()
            .filter(|p| self.get(**p).configured)
            .count()
    }

    /// 登録時に課す対策 (= effective な対策のみ)。
    pub fn effective_protections(&self) -> Vec<Protection> {
        Protection::ALL
            .into_iter()
            .filter(|p| self.get(*p).is_effective())
            .collect()
    }

    /// 設定上有効だが effective でない対策 (q07 の防御低下対象)。
    pub fn degraded_protections(&self) -> Vec<Protection> {
        Protection::ALL
            .into_iter()
            .filter(|p| {
                let observation = self.get(*p);
                observation.configured && !observation.is_effective()
            })
            .collect()
    }

    /// 最低条件 (q01): いずれかの対策が effective。
    pub fn meets_minimum(&self) -> bool {
        Protection::ALL.iter().any(|p| self.get(*p).is_effective())
    }
}

fn verified_by_legacy_default() -> bool {
    true
}

/// 管理者の開閉状態。初期状態は Closed。手動閉鎖は管理者のみが解除する (q03)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminState {
    Closed,
    Open,
}

/// 継続中の健全性 episode。通知イベントの重複排除は episode の変化でのみ新規生成することで実現する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthEpisode {
    None,
    /// 設定を失わない一時障害で effective な対策が 0 件 (q06)。
    Outage,
    /// 片方のみ障害 (q07)。
    Degraded,
}

/// override の有効中情報 (q09/q10)。管理画面の常時警告表示 (F15) が参照する。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OverrideInfo {
    pub activated_by: AuthAccountId,
    pub activated_at: OffsetDateTime,
}

/// 登録ゲートの永続状態 (singleton)。`revision` は楽観ロック用。
///
/// 不変条件:
/// - override 有効中は `config_lost_latch == false` かつ `episode == None`
/// - `revision` は admin / override / latch / episode / enforcement 照合状態の変更で +1。
///   provider 観測だけの反復では増えない
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrationGateState {
    pub admin: AdminState,
    pub override_info: Option<OverrideInfo>,
    pub config_lost_latch: bool,
    #[serde(default)]
    pub enforcement_lost_latch: bool,
    pub episode: HealthEpisode,
    pub observation: RegistrationObservation,
    pub revision: i64,
    pub updated_at: OffsetDateTime,
}

impl RegistrationGateState {
    /// 初期状態: Closed / override なし / ラッチなし / 観測なし。
    pub fn initial(now: OffsetDateTime) -> Self {
        Self {
            admin: AdminState::Closed,
            override_info: None,
            config_lost_latch: false,
            enforcement_lost_latch: false,
            episode: HealthEpisode::None,
            observation: RegistrationObservation::UNCONFIGURED,
            revision: 0,
            updated_at: now,
        }
    }

    pub fn is_override_active(&self) -> bool {
        self.override_info.is_some()
    }

    pub fn control_snapshot(&self) -> GateControlSnapshot {
        GateControlSnapshot {
            admin: self.admin,
            override_active: self.is_override_active(),
            config_lost_latch: self.config_lost_latch,
            enforcement_lost_latch: self.enforcement_lost_latch,
            enforcement_verified: self.observation.enforcement_verified,
            episode: self.episode,
            revision: self.revision,
        }
    }
}

/// 監査記録に残す制御状態。provider 観測は別記し、enforcement 照合状態はここにも残す。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateControlSnapshot {
    #[serde(default = "verified_by_legacy_default")]
    pub enforcement_verified: bool,
    pub admin: AdminState,
    pub override_active: bool,
    pub config_lost_latch: bool,
    #[serde(default)]
    pub enforcement_lost_latch: bool,
    pub episode: HealthEpisode,
    pub revision: i64,
}

/// 新規登録に対する判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum RegistrationDecision {
    /// 受付。`enforced` は登録時に課す対策 (effective なもののみ)。
    /// `degraded` は設定済みだが非 effective な対策 (防御低下。情報)。
    Accept {
        enforced: Vec<Protection>,
        degraded: Vec<Protection>,
        override_active: bool,
    },
    /// 管理者の手動閉鎖。
    AdminClosed,
    /// 設定喪失による停止ラッチ。復旧しても自動では解除されず、管理者の手動再開が必要。
    Stopped,
    /// 一時障害による一時拒否。復旧観測で自動的に Accept へ戻る。
    TemporaryReject,
}

impl RegistrationDecision {
    pub fn accepts(&self) -> bool {
        matches!(self, RegistrationDecision::Accept { .. })
    }
}
