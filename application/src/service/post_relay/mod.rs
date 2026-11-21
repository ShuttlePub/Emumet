//! Transaction participation facade for I1/S1. It never starts a sender or opens a second
//! transaction. Receipt/Undo writes and relay reservation must share the supplied connection.
pub mod delivery;
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::post_relay::{
    Admission, DependOnPostRelayStore, PostRelayStore, RelayAdmission,
};
use kernel::KernelError;
pub trait AdmitPostRelayUseCase: DependOnPostRelayStore {
    fn admit_post_relay(
        &self,
        connection: &mut <Self::DatabaseConnection as DatabaseConnection>::Connection,
        request: &RelayAdmission,
    ) -> impl std::future::Future<Output = error_stack::Result<Admission, KernelError>> + Send {
        self.post_relay_store().enqueue(connection, request)
    }
}
impl<T: DependOnPostRelayStore> AdmitPostRelayUseCase for T {}
