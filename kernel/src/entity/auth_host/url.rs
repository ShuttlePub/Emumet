use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AuthHostUrl(String);

impl AuthHostUrl {
    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.trim().is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Auth host URL cannot be empty"));
        }
        if !self.0.starts_with("http://") && !self.0.starts_with("https://") {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Auth host URL must start with http:// or https://"));
        }
        Ok(())
    }
}
