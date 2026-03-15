use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct RemoteAccountUrl(String);

impl RemoteAccountUrl {
    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.trim().is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Remote account URL cannot be empty"));
        }
        if !self.0.starts_with("http://") && !self.0.starts_with("https://") {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Remote account URL must start with http:// or https://"));
        }
        Ok(())
    }
}
