use crate::entity::{GraphActivityId, GraphActorUrl, ProfileId};
use crate::KernelError;
use error_stack::Report;
use time::OffsetDateTime;

const MAX_ACTIVITY_TYPE_LEN: usize = 64;

/// 受信した Activity の記録 (重複検知用)。
///
/// 重複判定のキーは `(receiver, actor, activity_id)`。activity ID を actor ごとに分けるのは、
/// 別 actor が他者の activity ID を名乗って本物の受信を潰す経路をなくすため。
/// 受信側 Profile ごとに記録するので、共有 inbox で複数 Profile 宛てに届いた同一 activity も
/// Profile ごとに 1 回ずつ処理できる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewInboundActivity {
    pub receiver: ProfileId,
    pub actor: GraphActorUrl,
    pub activity_id: GraphActivityId,
    pub activity_type: String,
    pub received_at: OffsetDateTime,
}

impl NewInboundActivity {
    pub fn new(
        receiver: ProfileId,
        actor: GraphActorUrl,
        activity_id: GraphActivityId,
        activity_type: impl Into<String>,
        received_at: OffsetDateTime,
    ) -> error_stack::Result<Self, KernelError> {
        let activity_type = activity_type.into();
        if activity_type.is_empty()
            || activity_type.len() > MAX_ACTIVITY_TYPE_LEN
            || !activity_type.chars().all(|c| c.is_ascii_alphanumeric())
        {
            return Err(Report::new(KernelError::Validation)
                .attach_printable("invalid inbound activity type"));
        }
        Ok(Self {
            receiver,
            actor,
            activity_id,
            activity_type,
            received_at,
        })
    }
}

/// 保存済みの受信記録。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundActivityRecord {
    pub receiver: ProfileId,
    pub actor: GraphActorUrl,
    pub activity_id: GraphActivityId,
    pub activity_type: String,
    /// 受信回数 (初回は 1。再受信のたびに増える)。
    pub receive_count: i32,
    pub first_received_at: OffsetDateTime,
    pub last_received_at: OffsetDateTime,
}

/// `InboundActivityRepository::record` の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundActivityReceipt {
    /// 初めて受信した activity なら true。false なら重複受信 (処理済みとして扱ってよい)。
    pub first_seen: bool,
    pub receive_count: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_activity_type() {
        let make = |t: &str| {
            NewInboundActivity::new(
                ProfileId::new(1),
                GraphActorUrl::new("https://remote.example/u").unwrap(),
                GraphActivityId::new("https://remote.example/a/1").unwrap(),
                t,
                OffsetDateTime::now_utc(),
            )
        };
        assert!(make("Follow").is_ok());
        assert!(make("").is_err());
        assert!(make("Fol low").is_err());
        assert!(make(&"a".repeat(65)).is_err());
    }
}
