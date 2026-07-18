use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct MetadataContent(String);

impl MetadataContent {
    pub const MAX_LENGTH: usize = 1000;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Field content must not exceed {} characters",
                    Self::MAX_LENGTH
                )),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_content() {
        let content = MetadataContent::new("https://example.com");
        assert!(content.validate().is_ok());
    }

    #[test]
    fn test_content_at_limit() {
        let content = MetadataContent::new("a".repeat(MetadataContent::MAX_LENGTH));
        assert!(content.validate().is_ok());
    }

    #[test]
    fn test_content_at_limit_multibyte_chars() {
        let content = MetadataContent::new("あ".repeat(MetadataContent::MAX_LENGTH));
        assert!(content.validate().is_ok());
    }

    #[test]
    fn test_content_over_limit() {
        let content = MetadataContent::new("a".repeat(MetadataContent::MAX_LENGTH + 1));
        let result = content.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Field content must not exceed 1000 characters"));
    }
}
