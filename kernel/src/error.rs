use error_stack::Context;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum KernelError {
    Concurrency,
    Timeout,
    Internal,
}

impl Display for KernelError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            KernelError::Concurrency => write!(f, "Concurrency error"),
            KernelError::Timeout => write!(f, "Process Timed out"),
            KernelError::Internal => write!(f, "Internal kernel error"),
        }
    }
}

impl Context for KernelError {}
