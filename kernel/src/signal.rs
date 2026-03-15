use crate::KernelError;
use std::future::Future;

pub trait Signal<ID> {
    fn emit(
        &self,
        signal_id: ID,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}
