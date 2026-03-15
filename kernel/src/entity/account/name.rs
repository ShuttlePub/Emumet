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
        Ok(())
    }
}
