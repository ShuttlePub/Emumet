use crate::entity::{AccountId, OutboxActivityId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutboxActivity {
    pub id: OutboxActivityId,
    pub account_id: AccountId,
    pub activity_id: String,
    pub activity_type: String,
    pub object_json: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}
