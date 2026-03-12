use kernel::prelude::entity::Metadata;

#[derive(Debug)]
pub struct MetadataDto {
    pub account_nanoid: String,
    pub nanoid: String,
    pub label: String,
    pub content: String,
}

impl MetadataDto {
    pub fn new(metadata: Metadata, account_nanoid: String) -> Self {
        Self {
            account_nanoid,
            nanoid: metadata.nanoid().as_ref().to_string(),
            label: metadata.label().as_ref().to_string(),
            content: metadata.content().as_ref().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::prelude::entity::{
        AccountId, EventVersion, Metadata, MetadataContent, MetadataId, MetadataLabel, Nanoid,
    };
    use uuid::Uuid;

    #[test]
    fn test_metadata_dto_new() {
        let metadata_id = MetadataId::new(Uuid::now_v7());
        let account_id = AccountId::new(Uuid::now_v7());
        let label = MetadataLabel::new("test label".to_string());
        let content = MetadataContent::new("test content".to_string());
        let nanoid = Nanoid::default();
        let version = EventVersion::new(Uuid::now_v7());
        let account_nanoid = "acc-nanoid-789".to_string();

        let metadata = Metadata::new(
            metadata_id,
            account_id,
            label.clone(),
            content.clone(),
            version,
            nanoid.clone(),
        );

        let dto = MetadataDto::new(metadata, account_nanoid.clone());

        assert_eq!(dto.account_nanoid, account_nanoid);
        assert_eq!(dto.nanoid, nanoid.as_ref().to_string());
        assert_eq!(dto.label, label.as_ref().to_string());
        assert_eq!(dto.content, content.as_ref().to_string());
    }
}
