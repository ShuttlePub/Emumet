use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::entity::GraphActorUrl;
use crate::KernelError;
use error_stack::Report;
use std::future::Future;
use time::OffsetDateTime;

/// `preferredUsername` の最大文字数。
pub const MAX_PREFERRED_USERNAME_CHARS: usize = 256;
/// 表示名の最大文字数 (超過分は切り詰める)。
pub const MAX_DISPLAY_NAME_CHARS: usize = 200;
/// icon URL の最大バイト数 (保存列の CHECK と同じ)。
pub const MAX_ICON_URL_BYTES: usize = 1024;

/// remote actor の表示用情報。actor 文書から取り出して検証・正規化した値だけを持つ (HTML ではなく plain text)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteActorProfile {
    preferred_username: String,
    display_name: Option<String>,
    icon_url: Option<String>,
}

fn rejected(message: &str) -> Report<KernelError> {
    Report::new(KernelError::Rejected).attach_printable(message.to_string())
}

/// 双方向制御文字 (表示の並びを入れ替えられる)。通常の制御文字とあわせて表示名から除く。
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

/// 表示されない書式文字 (ゼロ幅文字・BOM・soft hyphen など)。見た目が同じ別の文字列を作れるので除く。
fn is_invisible_format(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200D}' | '\u{2060}'..='\u{2064}' | '\u{FEFF}' | '\u{00AD}')
}

/// 表示の並びを入れ替えたり見えなくしたりする文字 (制御文字・双方向制御文字・不可視の書式文字)。
fn is_unsafe_display_char(c: char) -> bool {
    c.is_control() || is_bidi_control(c) || is_invisible_format(c)
}

fn sanitize_display_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !is_unsafe_display_char(*c))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_DISPLAY_NAME_CHARS).collect())
}

/// userinfo なしの絶対 http/https URL で `MAX_ICON_URL_BYTES` 以内ならその正規形を返す。
fn valid_icon_url(raw: &str) -> Option<String> {
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let text = url.to_string();
    (text.len() <= MAX_ICON_URL_BYTES).then_some(text)
}

/// `icon` の取り得る形 (文字列・`{url}`・`{url: {href}}`・それらの配列) から URL 候補を順に集める。
fn collect_icon_candidates<'a>(value: &'a serde_json::Value, out: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(text) => out.push(text),
        serde_json::Value::Array(items) => {
            for item in items {
                collect_icon_candidates(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            if let Some(url) = map.get("url") {
                collect_icon_candidates(url, out);
            }
            if let Some(serde_json::Value::String(href)) = map.get("href") {
                out.push(href);
            }
        }
        _ => {}
    }
}

impl RemoteActorProfile {
    /// 値から作る。`preferred_username` は空・空白・制御文字・双方向制御文字・不可視の書式文字・`@`・`/` を含まない 1〜256 文字でなければ
    /// `Rejected`。`display_name` と `icon_url` は不正なら捨てる (null にする。失敗にはしない)。
    pub fn new(
        preferred_username: &str,
        display_name: Option<&str>,
        icon_url: Option<&str>,
    ) -> error_stack::Result<Self, KernelError> {
        let chars = preferred_username.chars().count();
        if chars == 0
            || chars > MAX_PREFERRED_USERNAME_CHARS
            || preferred_username
                .chars()
                .any(|c| c.is_whitespace() || is_unsafe_display_char(c) || c == '@' || c == '/')
        {
            return Err(rejected("actor preferredUsername is not usable"));
        }
        Ok(Self {
            preferred_username: preferred_username.to_string(),
            display_name: display_name.and_then(sanitize_display_name),
            icon_url: icon_url.and_then(valid_icon_url),
        })
    }

    /// actor 文書 (JSON) の `preferredUsername` / `name` / `icon` から作る。
    pub fn from_actor_document(
        document: &serde_json::Value,
    ) -> error_stack::Result<Self, KernelError> {
        let username = document
            .get("preferredUsername")
            .and_then(|value| value.as_str())
            .ok_or_else(|| rejected("actor document has no preferredUsername"))?;
        let name = document.get("name").and_then(|value| value.as_str());
        let mut candidates = Vec::new();
        if let Some(icon) = document.get("icon") {
            collect_icon_candidates(icon, &mut candidates);
        }
        let icon = candidates.into_iter().find_map(valid_icon_url);
        Self::new(username, name, icon.as_deref())
    }

    /// 保存済みの値の復元 (保存前に検証済みなので再検証しない)。
    pub fn restore(
        preferred_username: String,
        display_name: Option<String>,
        icon_url: Option<String>,
    ) -> Self {
        Self {
            preferred_username,
            display_name,
            icon_url,
        }
    }

    pub fn preferred_username(&self) -> &str {
        &self.preferred_username
    }

    pub fn display_name(&self) -> Option<&str> {
        self.display_name.as_deref()
    }

    pub fn icon_url(&self) -> Option<&str> {
        self.icon_url.as_deref()
    }
}

/// キャッシュ行の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteActorCacheStatus {
    /// 未取得 (取得のリースを確保しただけ)。
    Pending,
    /// 取得済み。
    Resolved,
    /// 直近の取得が失敗 (表示情報なし)。
    Failed,
}

/// remote actor 1 件のキャッシュ行。`profile` は `Resolved` のときだけ `Some`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteActorCacheEntry {
    pub actor_url: GraphActorUrl,
    pub status: RemoteActorCacheStatus,
    pub profile: Option<RemoteActorProfile>,
    /// 次に取得してよい時刻 (取得中はリースの期限)。
    pub next_refresh_at: OffsetDateTime,
}

/// remote actor の表示情報キャッシュの保存 port。公開情報だけで、所有者・関係とは無関係。
pub trait ProfileRemoteActorCacheRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// 指定した actor URL のキャッシュ行 (無いものは含まれない)。順序は不定。
    fn find_many(
        &self,
        executor: &mut Self::Connection,
        actor_urls: &[GraphActorUrl],
    ) -> impl Future<Output = error_stack::Result<Vec<RemoteActorCacheEntry>, KernelError>> + Send;

    /// 取得のリースを原子的に確保する。行が無いか `next_refresh_at <= now` のときだけ
    /// `next_refresh_at = lease_until` にして true (行が無ければ `pending` 行を作る)。それ以外は false。
    /// 複数の呼び出し (別 transaction・別プロセス) が同時に要求しても true は 1 つだけ。
    fn try_claim_refresh(
        &self,
        executor: &mut Self::Connection,
        actor_url: &GraphActorUrl,
        now: OffsetDateTime,
        lease_until: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    /// 取得の成功を記録する (`resolved`・値・`fetched_at = now`・`next_refresh_at`)。
    fn record_resolved(
        &self,
        executor: &mut Self::Connection,
        actor_url: &GraphActorUrl,
        profile: &RemoteActorProfile,
        now: OffsetDateTime,
        next_refresh_at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// 取得の失敗を記録する。既に `resolved` の行はデータと status を保ち `next_refresh_at` だけ更新する
    /// (古い情報を返し続ける)。それ以外は `failed`。
    fn record_failed(
        &self,
        executor: &mut Self::Connection,
        actor_url: &GraphActorUrl,
        next_refresh_at: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnProfileRemoteActorCacheRepository:
    Sync + Send + DependOnDatabaseConnection
{
    type ProfileRemoteActorCacheRepository: ProfileRemoteActorCacheRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn profile_remote_actor_cache_repository(&self) -> &Self::ProfileRemoteActorCacheRepository;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn profile(
        document: serde_json::Value,
    ) -> error_stack::Result<RemoteActorProfile, KernelError> {
        RemoteActorProfile::from_actor_document(&document)
    }

    #[test]
    fn a_full_document_gives_every_field() {
        let p = profile(json!({
            "preferredUsername": "alice",
            "name": "Alice",
            "icon": {"type": "Image", "url": "https://remote.example/a.png"},
        }))
        .unwrap();
        assert_eq!(p.preferred_username(), "alice");
        assert_eq!(p.display_name(), Some("Alice"));
        assert_eq!(p.icon_url(), Some("https://remote.example/a.png"));
    }

    #[test]
    fn icon_accepts_string_object_href_and_array_forms() {
        for icon in [
            json!("https://r.example/i.png"),
            json!({"url": "https://r.example/i.png"}),
            json!({"url": {"href": "https://r.example/i.png"}}),
            json!([{"url": "javascript:alert(1)"}, {"url": "https://r.example/i.png"}]),
            json!([{"type": "Image"}, "https://r.example/i.png"]),
        ] {
            let p = profile(json!({"preferredUsername": "a", "icon": icon})).unwrap();
            assert_eq!(p.icon_url(), Some("https://r.example/i.png"), "{icon}");
        }
    }

    #[test]
    fn unusable_icons_become_null_without_failing() {
        for icon in [
            json!("javascript:alert(1)"),
            json!("data:image/png;base64,AAAA"),
            json!("ftp://r.example/i.png"),
            json!("/relative/i.png"),
            json!("https://user:pass@r.example/i.png"),
            json!("https://user@r.example/i.png"),
            json!(format!(
                "https://r.example/{}",
                "a".repeat(MAX_ICON_URL_BYTES)
            )),
            json!(42),
            json!({"url": null}),
            json!([]),
        ] {
            let p = profile(json!({"preferredUsername": "a", "icon": icon})).unwrap();
            assert_eq!(p.icon_url(), None, "{icon}");
        }
        assert_eq!(
            profile(json!({"preferredUsername": "a"}))
                .unwrap()
                .icon_url(),
            None
        );
    }

    #[test]
    fn display_name_is_cleaned_trimmed_and_truncated() {
        let name = |raw: &str| {
            profile(json!({"preferredUsername": "a", "name": raw}))
                .unwrap()
                .display_name()
                .map(str::to_string)
        };
        assert_eq!(
            name("  Alice\u{0}\n\t Smith \u{202E}"),
            Some("Alice Smith".to_string())
        );
        assert_eq!(name(""), None);
        assert_eq!(name("  \n "), None);
        let long = "あ".repeat(MAX_DISPLAY_NAME_CHARS + 50);
        assert_eq!(name(&long).unwrap().chars().count(), MAX_DISPLAY_NAME_CHARS);
        assert_eq!(
            profile(json!({"preferredUsername": "a", "name": 5}))
                .unwrap()
                .display_name(),
            None
        );
    }

    #[test]
    fn preferred_username_must_be_a_plain_handle() {
        for ok in [
            "alice",
            "a.b-c_d",
            "ゆーざー",
            &"a".repeat(MAX_PREFERRED_USERNAME_CHARS),
        ] {
            assert!(RemoteActorProfile::new(ok, None, None).is_ok(), "{ok}");
        }
        for bad in [
            "",
            " ",
            "a b",
            " alice",
            "alice ",
            "a@b",
            "a/b",
            "a\nb",
            "a\u{0}",
            // 双方向制御文字・不可視の書式文字は acct の表示を偽装できるので拒否する。
            "evil\u{202E}gpj",
            "a\u{202A}b",
            "a\u{2066}b",
            "a\u{200F}",
            "a\u{061C}",
            "a\u{200B}b",
            "a\u{FEFF}",
            "a\u{2060}b",
            &"a".repeat(MAX_PREFERRED_USERNAME_CHARS + 1),
        ] {
            let error = RemoteActorProfile::new(bad, None, None).unwrap_err();
            assert_eq!(error.current_context(), &KernelError::Rejected, "{bad:?}");
        }
        assert!(profile(json!({})).is_err());
        assert!(profile(json!({"preferredUsername": 3})).is_err());
    }
}
