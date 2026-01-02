mod crypto;
mod database;
mod entity;
mod error;
mod event;
mod modify;
mod query;
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

    pub mod signal {
        pub use crate::signal::*;
    }
}

/// Macro to delegate database-related DependOn* traits to a field.
///
/// This macro generates implementations for:
/// - DependOnDatabaseConnection
/// - DependOnAccountQuery, DependOnAuthAccountQuery, DependOnAuthHostQuery, DependOnEventQuery
/// - DependOnAccountModifier, DependOnAuthAccountModifier, DependOnAuthHostModifier, DependOnEventModifier
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

        impl $crate::interfaces::query::DependOnAccountQuery for $impl_type {
            type AccountQuery = <$db_type as $crate::interfaces::query::DependOnAccountQuery>::AccountQuery;
            fn account_query(&self) -> &Self::AccountQuery {
                $crate::interfaces::query::DependOnAccountQuery::account_query(&self.$field)
            }
        }

        impl $crate::interfaces::query::DependOnAuthAccountQuery for $impl_type {
            type AuthAccountQuery = <$db_type as $crate::interfaces::query::DependOnAuthAccountQuery>::AuthAccountQuery;
            fn auth_account_query(&self) -> &Self::AuthAccountQuery {
                $crate::interfaces::query::DependOnAuthAccountQuery::auth_account_query(&self.$field)
            }
        }

        impl $crate::interfaces::query::DependOnAuthHostQuery for $impl_type {
            type AuthHostQuery = <$db_type as $crate::interfaces::query::DependOnAuthHostQuery>::AuthHostQuery;
            fn auth_host_query(&self) -> &Self::AuthHostQuery {
                $crate::interfaces::query::DependOnAuthHostQuery::auth_host_query(&self.$field)
            }
        }

        impl $crate::interfaces::query::DependOnEventQuery for $impl_type {
            type EventQuery = <$db_type as $crate::interfaces::query::DependOnEventQuery>::EventQuery;
            fn event_query(&self) -> &Self::EventQuery {
                $crate::interfaces::query::DependOnEventQuery::event_query(&self.$field)
            }
        }

        impl $crate::interfaces::modify::DependOnAccountModifier for $impl_type {
            type AccountModifier = <$db_type as $crate::interfaces::modify::DependOnAccountModifier>::AccountModifier;
            fn account_modifier(&self) -> &Self::AccountModifier {
                $crate::interfaces::modify::DependOnAccountModifier::account_modifier(&self.$field)
            }
        }

        impl $crate::interfaces::modify::DependOnAuthAccountModifier for $impl_type {
            type AuthAccountModifier = <$db_type as $crate::interfaces::modify::DependOnAuthAccountModifier>::AuthAccountModifier;
            fn auth_account_modifier(&self) -> &Self::AuthAccountModifier {
                $crate::interfaces::modify::DependOnAuthAccountModifier::auth_account_modifier(&self.$field)
            }
        }

        impl $crate::interfaces::modify::DependOnAuthHostModifier for $impl_type {
            type AuthHostModifier = <$db_type as $crate::interfaces::modify::DependOnAuthHostModifier>::AuthHostModifier;
            fn auth_host_modifier(&self) -> &Self::AuthHostModifier {
                $crate::interfaces::modify::DependOnAuthHostModifier::auth_host_modifier(&self.$field)
            }
        }

        impl $crate::interfaces::modify::DependOnEventModifier for $impl_type {
            type EventModifier = <$db_type as $crate::interfaces::modify::DependOnEventModifier>::EventModifier;
            fn event_modifier(&self) -> &Self::EventModifier {
                $crate::interfaces::modify::DependOnEventModifier::event_modifier(&self.$field)
            }
        }
    };
}
