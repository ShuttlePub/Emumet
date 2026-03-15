use crate::error::ErrorStatus;
use application::transfer::pagination::Direction;
use axum::http::StatusCode;

pub mod account;
pub mod metadata;
pub mod oauth2;
pub mod profile;

const MAX_BATCH_SIZE: usize = 100;

fn parse_comma_ids(raw: &str) -> Result<Vec<String>, ErrorStatus> {
    let ids: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if ids.is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "ID list cannot be empty".to_string(),
        )));
    }
    if ids.len() > MAX_BATCH_SIZE {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            format!("Too many IDs: maximum is {MAX_BATCH_SIZE}"),
        )));
    }
    Ok(ids)
}

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
