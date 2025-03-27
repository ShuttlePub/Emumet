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
