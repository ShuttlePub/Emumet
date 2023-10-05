mod entity;
mod error;
mod repository;

#[cfg(feature = "interfaces")]
pub mod interfaces {
    pub mod repository {
        pub use crate::repository::*;
    }
}
