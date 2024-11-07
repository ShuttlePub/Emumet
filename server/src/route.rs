use crate::error::ErrorStatus;
use application::service::Direction;
use axum::http::StatusCode;

pub mod account;

trait DirectionConverter {
    fn convert_to_direction(self) -> Result<Option<Direction>, ErrorStatus>;
}

impl DirectionConverter for Option<String> {
    fn convert_to_direction(self) -> Result<Option<Direction>, ErrorStatus> {
        match self {
            Some(d) => match Direction::try_from(d) {
                Ok(d) => Ok(Some(d)),
                Err(message) => Err((StatusCode::BAD_REQUEST, message).into()),
            },
            None => Ok(None),
        }
    }
}
