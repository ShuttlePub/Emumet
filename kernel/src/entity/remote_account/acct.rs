use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

use crate::KernelError;

/// Acct means webfinger url like: `username@domain`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Deserialize, Serialize)]
pub struct RemoteAccountAcct(String);

impl RemoteAccountAcct {
    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.trim().is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Remote account acct cannot be empty"));
        }
        let parts: Vec<&str> = self.0.split('@').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return Err(Report::new(KernelError::Rejected)
                .attach_printable("Remote account acct must be in format 'username@domain'"));
        }
        Ok(())
    }
}
