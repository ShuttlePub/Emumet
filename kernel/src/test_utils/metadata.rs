use crate::entity::{
    AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
};

use super::{DEFAULT_METADATA_CONTENT, DEFAULT_METADATA_LABEL};

pub struct MetadataBuilder {
    id: Option<MetadataId>,
    account_id: Option<AccountId>,
    label: Option<MetadataLabel>,
    content: Option<MetadataContent>,
    version: Option<EventVersion<Metadata>>,
    nanoid: Option<Nanoid<Metadata>>,
}

impl Default for MetadataBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MetadataBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            account_id: None,
            label: None,
            content: None,
            version: None,
            nanoid: None,
        }
    }

    pub fn id(mut self, id: MetadataId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn account_id(mut self, account_id: AccountId) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(MetadataLabel::new(label));
        self
    }

    pub fn content(mut self, content: impl Into<String>) -> Self {
        self.content = Some(MetadataContent::new(content));
        self
    }

    pub fn version(mut self, version: EventVersion<Metadata>) -> Self {
        self.version = Some(version);
        self
    }

    pub fn nanoid(mut self, nanoid: Nanoid<Metadata>) -> Self {
        self.nanoid = Some(nanoid);
        self
    }

    pub fn build(self) -> Metadata {
        crate::ensure_generator_initialized();
        Metadata::new(
            self.id
                .unwrap_or_else(|| MetadataId::new(crate::generate_id())),
            self.account_id.unwrap_or_default(),
            self.label
                .unwrap_or_else(|| MetadataLabel::new(DEFAULT_METADATA_LABEL)),
            self.content
                .unwrap_or_else(|| MetadataContent::new(DEFAULT_METADATA_CONTENT)),
            self.version.unwrap_or_default(),
            self.nanoid.unwrap_or_default(),
        )
    }
}
