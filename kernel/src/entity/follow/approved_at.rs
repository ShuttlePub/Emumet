use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use vodca::{AsRefln, Fromln, Newln};

#[derive(
    Debug,
    Clone,
    Ord,
    PartialOrd,
    Eq,
    PartialEq,
    Hash,
    Fromln,
    AsRefln,
    Newln,
    Serialize,
    Deserialize,
)]
pub struct FollowApprovedAt(OffsetDateTime);

impl Default for FollowApprovedAt {
    fn default() -> Self {
        Self(OffsetDateTime::now_utc())
    }
}
