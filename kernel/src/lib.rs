mod entity;
mod error;
mod repository;

pub use self::error::KernelError;

#[cfg(feature = "prelude")]
pub mod prelude {
    pub mod entity {
        pub use crate::entity::*;
    }
}

#[cfg(feature = "interfaces")]
pub mod interfaces {
    pub mod repository {
        pub use crate::repository::*;
    }
}
