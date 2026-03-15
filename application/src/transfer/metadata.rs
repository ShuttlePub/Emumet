use kernel::prelude::entity::Metadata;

#[derive(Debug)]
pub struct CreateMetadataDto {
    pub account_nanoid: String,
    pub label: String,
    pub content: String,
}

#[derive(Debug)]
pub struct UpdateMetadataDto {
    pub account_nanoid: String,
    pub metadata_nanoid: String,
    pub label: String,
    pub content: String,
}

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
    use kernel::test_utils::{MetadataBuilder, DEFAULT_METADATA_CONTENT, DEFAULT_METADATA_LABEL};

    #[test]
    fn test_metadata_dto_new() {
        let metadata = MetadataBuilder::new().build();
        let account_nanoid = "acc-nanoid-789".to_string();
        let nanoid_str = metadata.nanoid().as_ref().to_string();

        let dto = MetadataDto::new(metadata, account_nanoid.clone());

        assert_eq!(dto.account_nanoid, account_nanoid);
        assert_eq!(dto.nanoid, nanoid_str);
        assert_eq!(dto.label, DEFAULT_METADATA_LABEL);
        assert_eq!(dto.content, DEFAULT_METADATA_CONTENT);
    }
}
