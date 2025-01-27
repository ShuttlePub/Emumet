use crate::error::ErrorStatus;
use application::transfer::pagination::Direction;
use axum::http::StatusCode;

pub mod account;

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

/// ("/a/b/c", "GET") -> `["/:GET", "/a:GET", "/a/b:GET", "/a/b/c:GET"]`
pub(super) fn to_permission_strings(path: &str, method: &str) -> Vec<String> {
    path.split('/')
        .scan(String::new(), |state, part| {
            if !state.is_empty() {
                state.push('/');
            }
            state.push_str(part);
            Some(format!("/{state}:{method}"))
        })
        .collect::<Vec<String>>()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_to_permission_strings() {
        let path = "/a/b/c";
        let method = "GET";
        let result = to_permission_strings(path, method);
        let expected = vec!["/:GET", "/a:GET", "/a/b:GET", "/a/b/c:GET"];
        assert_eq!(result, expected);
    }
}
