mod database;
mod entity;
mod error;
mod modify;
mod query;
mod repository;

pub use self::error::*;

#[cfg(feature = "prelude")]
pub mod prelude {
    pub mod entity {
        pub use crate::entity::*;
    }
}

#[cfg(feature = "interfaces")]
pub mod interfaces {
    pub mod database {
        pub use crate::database::*;
    }
    pub mod query {
        pub use crate::query::*;
    }
    pub mod modify {
        pub use crate::modify::*;
    }
    pub mod repository {
        pub use crate::repository::*;
    }
}
