use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct MetadataLabel(String);

impl MetadataLabel {
    pub const MAX_LENGTH: usize = 255;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Field label must not exceed {} characters",
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
    fn test_valid_label() {
        let label = MetadataLabel::new("Website");
        assert!(label.validate().is_ok());
    }

    #[test]
    fn test_label_at_limit() {
        let label = MetadataLabel::new("a".repeat(MetadataLabel::MAX_LENGTH));
        assert!(label.validate().is_ok());
    }

    #[test]
    fn test_label_at_limit_multibyte_chars() {
        let label = MetadataLabel::new("あ".repeat(MetadataLabel::MAX_LENGTH));
        assert!(label.validate().is_ok());
    }

    #[test]
    fn test_label_over_limit() {
        let label = MetadataLabel::new("a".repeat(MetadataLabel::MAX_LENGTH + 1));
        let result = label.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Field label must not exceed 255 characters"));
    }
}
