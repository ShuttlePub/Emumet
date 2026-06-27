#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetActorDto {
    pub account_nanoid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetWebFingerDto {
    pub account_name: String,
    pub domain: String,
}

#[derive(Debug, Clone)]
pub struct InboxActivityDto {
    pub account_id: kernel::prelude::entity::AccountId,
    pub account_nanoid: String,
    pub activity: kernel::activitypub::Activity,
}

#[derive(Debug, Clone)]
pub struct SendFollowDto {
    pub account_nanoid: String,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct SendFollowResultDto {
    pub follow_id: String,
    pub remote_actor_url: String,
    pub activity_id: String,
    pub approved: bool,
}
