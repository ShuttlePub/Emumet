use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AccountName(String);

impl AccountName {
    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.trim().is_empty() {
            return Err(
                Report::new(KernelError::Rejected).attach_printable("Account name cannot be empty")
            );
        }
        // ActivityPub preferredUsername / WebFinger localpart must not contain whitespace
        if self.0.contains(|c: char| c.is_whitespace()) {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Account name cannot contain whitespace"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_name() {
        let name = AccountName::new("alice");
        assert!(name.validate().is_ok());
    }

    #[test]
    fn test_valid_name_with_dot() {
        let name = AccountName::new("alice.smith");
        assert!(name.validate().is_ok());
    }

    #[test]
    fn test_valid_name_with_underscore() {
        let name = AccountName::new("alice_smith");
        assert!(name.validate().is_ok());
    }

    #[test]
    fn test_valid_name_with_digits_and_letters() {
        let name = AccountName::new("alice123");
        assert!(name.validate().is_ok());
    }

    #[test]
    fn test_empty_name() {
        let name = AccountName::new("");
        let result = name.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Account name cannot be empty"));
    }

    #[test]
    fn test_whitespace_only_name() {
        let name = AccountName::new("   ");
        let result = name.validate();
        assert!(result.is_err());
        // Should be caught by empty check (trim is empty)
        assert!(format!("{:?}", result).contains("Account name cannot be empty"));
    }

    #[test]
    fn test_name_with_leading_space() {
        let name = AccountName::new(" alice");
        let result = name.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Account name cannot contain whitespace"));
    }

    #[test]
    fn test_name_with_trailing_space() {
        let name = AccountName::new("alice ");
        let result = name.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Account name cannot contain whitespace"));
    }

    #[test]
    fn test_name_with_tab() {
        let name = AccountName::new("ali\tce");
        let result = name.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Account name cannot contain whitespace"));
    }

    #[test]
    fn test_name_with_inner_space() {
        let name = AccountName::new("ali ce");
        let result = name.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Account name cannot contain whitespace"));
    }
}
