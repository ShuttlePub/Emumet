mod postgres;
mod redis;

use error_stack::Report;
use kernel::KernelError;
use std::env;

pub use postgres::*;
pub use redis::*;

pub(crate) fn env(key: &str) -> error_stack::Result<Option<String>, KernelError> {
    let result = dotenvy::var(key);
    match result {
        Ok(var) => Ok(Some(var)),
        Err(dotenvy::Error::EnvVar(_)) => Ok(None),
        Err(error) => Err(Report::new(error).change_context(KernelError::Internal)),
    }
}
