use crate::entity::{AccountId, Block, BlockId, BlockTargetId};

pub struct BlockBuilder {
    id: Option<BlockId>,
    source: Option<BlockTargetId>,
    destination: Option<BlockTargetId>,
}

impl Default for BlockBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            source: None,
            destination: None,
        }
    }

    pub fn id(mut self, id: BlockId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn source(mut self, source: BlockTargetId) -> Self {
        self.source = Some(source);
        self
    }

    pub fn source_local(mut self, account_id: AccountId) -> Self {
        self.source = Some(BlockTargetId::Local(account_id));
        self
    }

    pub fn destination(mut self, destination: BlockTargetId) -> Self {
        self.destination = Some(destination);
        self
    }

    pub fn destination_local(mut self, account_id: AccountId) -> Self {
        self.destination = Some(BlockTargetId::Local(account_id));
        self
    }

    pub fn build(self) -> Block {
        crate::ensure_generator_initialized();
        let source = self
            .source
            .unwrap_or_else(|| BlockTargetId::Local(AccountId::default()));
        let destination = self
            .destination
            .unwrap_or_else(|| BlockTargetId::Local(AccountId::default()));
        Block::new(
            self.id
                .unwrap_or_else(|| BlockId::new(crate::generate_id())),
            source,
            destination,
        )
        .expect("Failed to build Block: both source and destination are remote")
    }
}
