#[derive(Debug, thiserror::Error)]
pub enum KernelError {
    #[error("cannot find `{id}:{entity}` in the following {method}.")]
    NotFound {
        method: &'static str,
        entity: &'static str,
        id: String,
    },
    #[error(transparent)]
    Serde(anyhow::Error),
    #[error(transparent)]
    Parse(anyhow::Error),
    #[error(transparent)]
    Driver(anyhow::Error),
    #[error(transparent)]
    External(anyhow::Error),
}