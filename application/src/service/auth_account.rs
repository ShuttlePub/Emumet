use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
use kernel::interfaces::query::{AuthAccountQuery, DependOnAuthAccountQuery};
use kernel::prelude::entity::{AuthAccount, AuthAccountClientId};
use kernel::KernelError;

pub(crate) async fn get_auth_account<
    S: DependOnDatabaseConnection + DependOnAuthAccountQuery + ?Sized,
>(
    service: &S,
    client_id: String,
) -> error_stack::Result<Option<AuthAccount>, KernelError> {
    let client_id = AuthAccountClientId::new(client_id);
    let mut transaction = service.database_connection().begin_transaction().await?;
    let auth_account = service
        .auth_account_query()
        .find_by_client_id(&mut transaction, &client_id)
        .await?;
    Ok(auth_account)
}
