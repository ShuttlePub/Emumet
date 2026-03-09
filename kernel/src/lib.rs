mod crypto;
mod database;
mod entity;
mod error;
mod event;
mod event_store;
mod modify;
mod query;
mod read_model;
mod signal;

pub use self::error::*;

#[cfg(feature = "prelude")]
pub mod prelude {
    pub mod entity {
        pub use crate::entity::*;
    }
}

#[cfg(feature = "interfaces")]
pub mod interfaces {
    pub mod crypto {
        pub use crate::crypto::*;
    }
    pub mod database {
        pub use crate::database::*;
    }
    pub mod query {
        pub use crate::query::*;
    }
    pub mod modify {
        pub use crate::modify::*;
    }
    pub mod event {
        pub use crate::event::*;
    }
    pub mod event_store {
        pub use crate::event_store::*;
    }
    pub mod read_model {
        pub use crate::read_model::*;
    }
    pub mod signal {
        pub use crate::signal::*;
    }
}

/// Macro to delegate database-related DependOn* traits to a field.
///
/// This macro generates implementations for:
/// - DependOnDatabaseConnection
/// - DependOnAccountReadModel, DependOnAccountEventStore
/// - DependOnAuthAccountReadModel, DependOnAuthAccountEventStore
/// - DependOnAuthHostQuery
/// - DependOnAuthHostModifier
///
/// # Usage
/// ```ignore
/// impl_database_delegation!(Handler, pgpool, PostgresDatabase);
/// ```
///
/// When switching DB implementation, only the macro arguments need to change:
/// ```ignore
/// impl_database_delegation!(Handler, db, MysqlDatabase);
/// ```
#[macro_export]
macro_rules! impl_database_delegation {
    ($impl_type:ty, $field:ident, $db_type:ty) => {
        impl $crate::interfaces::database::DependOnDatabaseConnection for $impl_type {
            type DatabaseConnection = $db_type;
            fn database_connection(&self) -> &Self::DatabaseConnection {
                &self.$field
            }
        }

        impl $crate::interfaces::read_model::DependOnAccountReadModel for $impl_type {
            type AccountReadModel = <$db_type as $crate::interfaces::read_model::DependOnAccountReadModel>::AccountReadModel;
            fn account_read_model(&self) -> &Self::AccountReadModel {
                $crate::interfaces::read_model::DependOnAccountReadModel::account_read_model(&self.$field)
            }
        }

        impl $crate::interfaces::event_store::DependOnAccountEventStore for $impl_type {
            type AccountEventStore = <$db_type as $crate::interfaces::event_store::DependOnAccountEventStore>::AccountEventStore;
            fn account_event_store(&self) -> &Self::AccountEventStore {
                $crate::interfaces::event_store::DependOnAccountEventStore::account_event_store(&self.$field)
            }
        }

        impl $crate::interfaces::read_model::DependOnAuthAccountReadModel for $impl_type {
            type AuthAccountReadModel = <$db_type as $crate::interfaces::read_model::DependOnAuthAccountReadModel>::AuthAccountReadModel;
            fn auth_account_read_model(&self) -> &Self::AuthAccountReadModel {
                $crate::interfaces::read_model::DependOnAuthAccountReadModel::auth_account_read_model(&self.$field)
            }
        }

        impl $crate::interfaces::event_store::DependOnAuthAccountEventStore for $impl_type {
            type AuthAccountEventStore = <$db_type as $crate::interfaces::event_store::DependOnAuthAccountEventStore>::AuthAccountEventStore;
            fn auth_account_event_store(&self) -> &Self::AuthAccountEventStore {
                $crate::interfaces::event_store::DependOnAuthAccountEventStore::auth_account_event_store(&self.$field)
            }
        }

        impl $crate::interfaces::query::DependOnAuthHostQuery for $impl_type {
            type AuthHostQuery = <$db_type as $crate::interfaces::query::DependOnAuthHostQuery>::AuthHostQuery;
            fn auth_host_query(&self) -> &Self::AuthHostQuery {
                $crate::interfaces::query::DependOnAuthHostQuery::auth_host_query(&self.$field)
            }
        }

        impl $crate::interfaces::modify::DependOnAuthHostModifier for $impl_type {
            type AuthHostModifier = <$db_type as $crate::interfaces::modify::DependOnAuthHostModifier>::AuthHostModifier;
            fn auth_host_modifier(&self) -> &Self::AuthHostModifier {
                $crate::interfaces::modify::DependOnAuthHostModifier::auth_host_modifier(&self.$field)
            }
        }

    };
}
