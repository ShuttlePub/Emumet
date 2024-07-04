mod postgres;

use error_stack::ResultExt;
use kernel::KernelError;
pub use postgres::*;

pub(crate) fn env(key: &str) -> error_stack::Result<String, KernelError> {
    dotenvy::var(key).change_context_lazy(|| KernelError::Internal)
}
