use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ModerationReason(String);

impl ModerationReason {
    pub const MAX_LENGTH: usize = 500;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.trim().is_empty() {
            return Err(Report::new(KernelError::Validation)
                .attach_printable("Reason cannot be empty".to_string()));
        }
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Reason must not exceed {} characters",
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
    fn test_valid_reason() {
        let reason = ModerationReason::new("spam");
        assert!(reason.validate().is_ok());
    }

    #[test]
    fn test_empty_reason() {
        let reason = ModerationReason::new("");
        let result = reason.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Reason cannot be empty"));
    }

    #[test]
    fn test_whitespace_only_reason() {
        let reason = ModerationReason::new("   ");
        let result = reason.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Reason cannot be empty"));
    }

    #[test]
    fn test_reason_at_limit() {
        let reason = ModerationReason::new("a".repeat(ModerationReason::MAX_LENGTH));
        assert!(reason.validate().is_ok());
    }

    #[test]
    fn test_reason_at_limit_multibyte_chars() {
        let reason = ModerationReason::new("あ".repeat(ModerationReason::MAX_LENGTH));
        assert!(reason.validate().is_ok());
    }

    #[test]
    fn test_reason_over_limit() {
        let reason = ModerationReason::new("a".repeat(ModerationReason::MAX_LENGTH + 1));
        let result = reason.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Reason must not exceed 500 characters"));
    }
}
