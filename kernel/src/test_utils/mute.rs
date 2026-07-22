use crate::entity::{AccountId, Mute, MuteId, MuteTargetId};

pub struct MuteBuilder {
    id: Option<MuteId>,
    source: Option<MuteTargetId>,
    destination: Option<MuteTargetId>,
}

impl Default for MuteBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MuteBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            source: None,
            destination: None,
        }
    }

    pub fn id(mut self, id: MuteId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn source(mut self, source: MuteTargetId) -> Self {
        self.source = Some(source);
        self
    }

    pub fn source_local(mut self, account_id: AccountId) -> Self {
        self.source = Some(MuteTargetId::Local(account_id));
        self
    }

    pub fn destination(mut self, destination: MuteTargetId) -> Self {
        self.destination = Some(destination);
        self
    }

    pub fn destination_local(mut self, account_id: AccountId) -> Self {
        self.destination = Some(MuteTargetId::Local(account_id));
        self
    }

    pub fn build(self) -> Mute {
        crate::ensure_generator_initialized();
        let source = self
            .source
            .unwrap_or_else(|| MuteTargetId::Local(AccountId::default()));
        let destination = self
            .destination
            .unwrap_or_else(|| MuteTargetId::Local(AccountId::default()));
        Mute::new(
            self.id.unwrap_or_else(|| MuteId::new(crate::generate_id())),
            source,
            destination,
        )
        .expect("Failed to build Mute: both source and destination are remote")
    }
}
