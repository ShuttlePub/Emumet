use kernel::KernelError;

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error(transparent)]
    SqlX(#[from] sqlx::Error),
    #[error(transparent)]
    Kernel(#[from] KernelError),
}

impl From<DriverError> for KernelError {
    fn from(value: DriverError) -> Self {
        match value {
            DriverError::SqlX(e) => KernelError::Driver(anyhow::Error::new(e)),
            DriverError::Kernel(e) => e,
        }
    }
}
