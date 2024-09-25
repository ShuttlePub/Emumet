mod postgres;

use error_stack::Report;
use kernel::KernelError;
pub use postgres::*;
use std::env;

pub(crate) fn env(key: &str) -> error_stack::Result<Option<String>, KernelError> {
    let result = dotenvy::var(key);
    match result {
        Ok(var) => Ok(Some(var)),
        Err(dotenvy::Error::EnvVar(_)) => Ok(None),
        Err(error) => Err(Report::new(error).change_context(KernelError::Internal)),
    }
}
