use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ProfileDisplayName(String);

impl ProfileDisplayName {
    pub const MAX_LENGTH: usize = 100;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Display name must not exceed {} characters",
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
    fn test_valid_display_name() {
        let name = ProfileDisplayName::new("Alice Wonderland");
        assert!(name.validate().is_ok());
    }

    #[test]
    fn test_display_name_at_limit() {
        let name = ProfileDisplayName::new("a".repeat(ProfileDisplayName::MAX_LENGTH));
        assert!(name.validate().is_ok());
    }

    #[test]
    fn test_display_name_at_limit_multibyte_chars() {
        let name = ProfileDisplayName::new("あ".repeat(ProfileDisplayName::MAX_LENGTH));
        assert!(name.validate().is_ok());
    }

    #[test]
    fn test_display_name_over_limit() {
        let name = ProfileDisplayName::new("a".repeat(ProfileDisplayName::MAX_LENGTH + 1));
        let result = name.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Display name must not exceed 100 characters"));
    }
}
