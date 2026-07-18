use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ImageUrl(String);

impl ImageUrl {
    pub const MAX_LENGTH: usize = 2048;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.is_empty() {
            return Err(Report::new(KernelError::Validation)
                .attach_printable("Image URL cannot be empty".to_string()));
        }
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Image URL must not exceed {} characters",
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
    fn test_valid_image_url() {
        let url = ImageUrl::new("https://img.example.com/avatar.png");
        assert!(url.validate().is_ok());
    }

    #[test]
    fn test_empty_image_url() {
        let url = ImageUrl::new("");
        let result = url.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Image URL cannot be empty"));
    }

    #[test]
    fn test_image_url_at_limit() {
        let url = ImageUrl::new("a".repeat(ImageUrl::MAX_LENGTH));
        assert!(url.validate().is_ok());
    }

    #[test]
    fn test_image_url_over_limit() {
        let url = ImageUrl::new("a".repeat(ImageUrl::MAX_LENGTH + 1));
        let result = url.validate();
        assert!(result.is_err());
        assert!(format!("{:?}", result).contains("Image URL must not exceed 2048 characters"));
    }
}
