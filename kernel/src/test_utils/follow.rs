use crate::entity::{AccountId, Follow, FollowApprovedAt, FollowId, FollowTargetId};

pub struct FollowBuilder {
    id: Option<FollowId>,
    source: Option<FollowTargetId>,
    destination: Option<FollowTargetId>,
    approved_at: Option<Option<FollowApprovedAt>>,
}

impl Default for FollowBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl FollowBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            source: None,
            destination: None,
            approved_at: None,
        }
    }

    pub fn id(mut self, id: FollowId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn source(mut self, source: FollowTargetId) -> Self {
        self.source = Some(source);
        self
    }

    pub fn source_local(mut self, account_id: AccountId) -> Self {
        self.source = Some(FollowTargetId::Local(account_id));
        self
    }

    pub fn destination(mut self, destination: FollowTargetId) -> Self {
        self.destination = Some(destination);
        self
    }

    pub fn destination_local(mut self, account_id: AccountId) -> Self {
        self.destination = Some(FollowTargetId::Local(account_id));
        self
    }

    pub fn approved_at(mut self, approved_at: Option<FollowApprovedAt>) -> Self {
        self.approved_at = Some(approved_at);
        self
    }

    pub fn build(self) -> Follow {
        crate::ensure_generator_initialized();
        let source = self
            .source
            .unwrap_or_else(|| FollowTargetId::Local(AccountId::default()));
        let destination = self
            .destination
            .unwrap_or_else(|| FollowTargetId::Local(AccountId::default()));
        Follow::new(
            self.id
                .unwrap_or_else(|| FollowId::new(crate::generate_id())),
            source,
            destination,
            self.approved_at.unwrap_or(None),
        )
        .expect("Failed to build Follow: both source and destination are remote")
    }
}
