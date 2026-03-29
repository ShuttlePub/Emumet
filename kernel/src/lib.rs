mod config;
mod crypto;
mod database;
mod entity;
mod error;
mod event;
mod event_store;
mod http_signing;
pub mod id;
mod permission;
mod read_model;
mod repository;
mod signal;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

pub use id::*;

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
    pub mod event {
        pub use crate::event::*;
    }
    pub mod event_store {
        pub use crate::event_store::*;
    }
    pub mod read_model {
        pub use crate::read_model::*;
    }
    pub mod repository {
        pub use crate::entity::{DependOnSigningKeyRepository, SigningKeyRepository};
        pub use crate::repository::*;
    }
    pub mod permission {
        pub use crate::permission::*;
    }
    pub mod signal {
        pub use crate::signal::*;
    }
    pub mod http_signing {
        pub use crate::http_signing::*;
    }
    pub mod config {
        pub use crate::config::*;
    }
}

/// Macro to delegate database-related DependOn* traits to a field.
///
/// This macro generates implementations for:
/// - DependOnDatabaseConnection
/// - DependOnAccountReadModel, DependOnAccountEventStore
/// - DependOnAuthAccountReadModel, DependOnAuthAccountEventStore
/// - DependOnProfileReadModel, DependOnProfileEventStore
/// - DependOnMetadataReadModel, DependOnMetadataEventStore
/// - DependOnAuthHostRepository
/// - DependOnFollowRepository
/// - DependOnRemoteAccountRepository
/// - DependOnImageRepository
/// - DependOnSigningKeyRepository
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

        impl $crate::interfaces::read_model::DependOnProfileReadModel for $impl_type {
            type ProfileReadModel = <$db_type as $crate::interfaces::read_model::DependOnProfileReadModel>::ProfileReadModel;
            fn profile_read_model(&self) -> &Self::ProfileReadModel {
                $crate::interfaces::read_model::DependOnProfileReadModel::profile_read_model(&self.$field)
            }
        }

        impl $crate::interfaces::event_store::DependOnProfileEventStore for $impl_type {
            type ProfileEventStore = <$db_type as $crate::interfaces::event_store::DependOnProfileEventStore>::ProfileEventStore;
            fn profile_event_store(&self) -> &Self::ProfileEventStore {
                $crate::interfaces::event_store::DependOnProfileEventStore::profile_event_store(&self.$field)
            }
        }

        impl $crate::interfaces::read_model::DependOnMetadataReadModel for $impl_type {
            type MetadataReadModel = <$db_type as $crate::interfaces::read_model::DependOnMetadataReadModel>::MetadataReadModel;
            fn metadata_read_model(&self) -> &Self::MetadataReadModel {
                $crate::interfaces::read_model::DependOnMetadataReadModel::metadata_read_model(&self.$field)
            }
        }

        impl $crate::interfaces::event_store::DependOnMetadataEventStore for $impl_type {
            type MetadataEventStore = <$db_type as $crate::interfaces::event_store::DependOnMetadataEventStore>::MetadataEventStore;
            fn metadata_event_store(&self) -> &Self::MetadataEventStore {
                $crate::interfaces::event_store::DependOnMetadataEventStore::metadata_event_store(&self.$field)
            }
        }

        impl $crate::interfaces::repository::DependOnAuthHostRepository for $impl_type {
            type AuthHostRepository = <$db_type as $crate::interfaces::repository::DependOnAuthHostRepository>::AuthHostRepository;
            fn auth_host_repository(&self) -> &Self::AuthHostRepository {
                $crate::interfaces::repository::DependOnAuthHostRepository::auth_host_repository(&self.$field)
            }
        }

        impl $crate::interfaces::repository::DependOnFollowRepository for $impl_type {
            type FollowRepository = <$db_type as $crate::interfaces::repository::DependOnFollowRepository>::FollowRepository;
            fn follow_repository(&self) -> &Self::FollowRepository {
                $crate::interfaces::repository::DependOnFollowRepository::follow_repository(&self.$field)
            }
        }

        impl $crate::interfaces::repository::DependOnRemoteAccountRepository for $impl_type {
            type RemoteAccountRepository = <$db_type as $crate::interfaces::repository::DependOnRemoteAccountRepository>::RemoteAccountRepository;
            fn remote_account_repository(&self) -> &Self::RemoteAccountRepository {
                $crate::interfaces::repository::DependOnRemoteAccountRepository::remote_account_repository(&self.$field)
            }
        }

        impl $crate::interfaces::repository::DependOnImageRepository for $impl_type {
            type ImageRepository = <$db_type as $crate::interfaces::repository::DependOnImageRepository>::ImageRepository;
            fn image_repository(&self) -> &Self::ImageRepository {
                $crate::interfaces::repository::DependOnImageRepository::image_repository(&self.$field)
            }
        }

        impl $crate::interfaces::repository::DependOnSigningKeyRepository for $impl_type {
            type SigningKeyRepository = <$db_type as $crate::interfaces::repository::DependOnSigningKeyRepository>::SigningKeyRepository;
            fn signing_key_repository(&self) -> &Self::SigningKeyRepository {
                $crate::interfaces::repository::DependOnSigningKeyRepository::signing_key_repository(&self.$field)
            }
        }

    };
}
