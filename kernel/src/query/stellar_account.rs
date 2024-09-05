use crate::database::{DependOnDatabaseConnection, Transaction};
use crate::entity::{EventEnvelope, StellarAccount, StellarAccountEvent, StellarAccountId};
use crate::KernelError;

pub trait StellarAccountQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &StellarAccountId,
    ) -> error_stack::Result<Option<StellarAccount>, KernelError>;
}

pub trait DependOnStellarAccountQuery: Sync + Send + DependOnDatabaseConnection {
    type StellarAccountQuery: StellarAccountQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn stellar_account_query(&self) -> &Self::StellarAccountQuery;
}

pub trait StellarAccountEventQuery: Sync + Send + 'static {
    type Transaction: Transaction;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &StellarAccountId,
    ) -> error_stack::Result<Vec<EventEnvelope<StellarAccountEvent, StellarAccount>>, KernelError>;
}

pub trait DependOnStellarAccountEventQuery: Sync + Send + DependOnDatabaseConnection {
    type StellarAccountEventQuery: StellarAccountEventQuery<
        Transaction = <Self::DatabaseConnection as crate::database::DatabaseConnection>::Transaction,
    >;

    fn stellar_account_event_query(&self) -> &Self::StellarAccountEventQuery;
}
