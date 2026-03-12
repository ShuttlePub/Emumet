use crate::error::ErrorStatus;
use application::transfer::pagination::Direction;
use axum::http::StatusCode;

pub mod account;
pub mod metadata;
pub mod oauth2;
pub mod profile;

trait DirectionConverter {
    fn convert_to_direction(self) -> Result<Direction, ErrorStatus>;
}

impl DirectionConverter for Option<String> {
    fn convert_to_direction(self) -> Result<Direction, ErrorStatus> {
        match self {
            Some(d) => match Direction::try_from(d) {
                Ok(d) => Ok(d),
                Err(message) => Err((StatusCode::BAD_REQUEST, message).into()),
            },
            None => Ok(Direction::default()),
        }
    }
}
