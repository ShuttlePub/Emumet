use crate::entity::{GraphActorUrl, ProfileId};
use serde::{Deserialize, Serialize};

/// 関係の当事者。ローカルは `ProfileId`、リモートは canonical actor URL だけで表す。
///
/// E8 の API 応答型 `ProfileActorRef` (`kind` / `actor_url` / `profile_id`) の保存側の対応物。
/// ローカル actor の URL は P1 が `ProfileId` から導出するので、ここには保存しない。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ProfileGraphActor {
    Local(ProfileId),
    Remote(GraphActorUrl),
}

impl ProfileGraphActor {
    pub fn local_profile_id(&self) -> Option<&ProfileId> {
        match self {
            Self::Local(profile_id) => Some(profile_id),
            Self::Remote(_) => None,
        }
    }

    pub fn remote_url(&self) -> Option<&GraphActorUrl> {
        match self {
            Self::Local(_) => None,
            Self::Remote(url) => Some(url),
        }
    }

    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }
}

impl From<ProfileId> for ProfileGraphActor {
    fn from(id: ProfileId) -> Self {
        Self::Local(id)
    }
}

impl From<GraphActorUrl> for ProfileGraphActor {
    fn from(url: GraphActorUrl) -> Self {
        Self::Remote(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_round_trip_for_both_variants() {
        let local = ProfileGraphActor::Local(ProfileId::new(42));
        let json = serde_json::to_string(&local).unwrap();
        assert_eq!(json, r#"{"type":"local","value":42}"#);
        assert_eq!(
            serde_json::from_str::<ProfileGraphActor>(&json).unwrap(),
            local
        );

        let remote =
            ProfileGraphActor::Remote(GraphActorUrl::new("https://remote.example/u/bob").unwrap());
        let json = serde_json::to_string(&remote).unwrap();
        assert_eq!(
            json,
            r#"{"type":"remote","value":"https://remote.example/u/bob"}"#
        );
        assert_eq!(
            serde_json::from_str::<ProfileGraphActor>(&json).unwrap(),
            remote
        );

        assert!(
            serde_json::from_str::<ProfileGraphActor>(r#"{"type":"remote","value":"nope"}"#)
                .is_err()
        );
    }
}
