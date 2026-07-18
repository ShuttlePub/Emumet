use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileSummary(String);

impl ProfileSummary {
    pub const MAX_LENGTH: usize = 2000;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Summary must not exceed {} characters",
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
    fn test_valid_summary() {
        let summary = ProfileSummary::new("Hello! I'm a test user on ShuttlePub.");
        assert!(summary.validate().is_ok());
    }

    #[test]
    fn test_summary_at_limit() {
        let summary = ProfileSummary::new("a".repeat(ProfileSummary::MAX_LENGTH));
        assert!(summary.validate().is_ok());
    }

    #[test]
    fn test_summary_at_limit_multibyte_chars() {
        let summary = ProfileSummary::new("あ".repeat(ProfileSummary::MAX_LENGTH));
        assert!(summary.validate().is_ok());
    }

    #[test]
    fn test_summary_over_limit() {
        let summary = ProfileSummary::new("a".repeat(ProfileSummary::MAX_LENGTH + 1));
        let result = summary.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Summary must not exceed 2000 characters"));
    }
}
