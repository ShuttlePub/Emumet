use vodca::Nameln;

#[derive(Debug, Nameln)]
#[vodca(snake_case)]
pub enum Direction {
    NEXT,
    PREV,
}

impl Default for Direction {
    fn default() -> Self {
        Self::NEXT
    }
}

impl TryFrom<String> for Direction {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "next" => Ok(Self::NEXT),
            "prev" => Ok(Self::PREV),
            other => Err(format!("Invalid direction: {}", other)),
        }
    }
}

#[derive(Debug)]
pub struct Pagination<Cursor> {
    pub limit: u32,
    pub cursor: Option<Cursor>,
    pub direction: Direction,
}

impl<Cursor> Pagination<Cursor> {
    pub fn new(limit: Option<u32>, cursor: Option<Cursor>, direction: Direction) -> Self {
        Self {
            limit: limit.unwrap_or(5),
            cursor,
            direction,
        }
    }
}

pub(crate) fn apply_pagination<T: Ord>(
    vec: Vec<T>,
    limit: u32,
    cursor_data: Option<T>,
    direction: Direction,
) -> Vec<T> {
    let mut vec = vec;
    match direction {
        Direction::NEXT => {
            vec.sort();
            vec = vec
                .into_iter()
                .filter(|x| {
                    cursor_data
                        .as_ref()
                        .map(|cursor_data| x > cursor_data)
                        .unwrap_or(true)
                })
                .take(limit as usize)
                .collect();
        }
        Direction::PREV => {
            vec.sort_by(|a, b| b.cmp(a));
            vec = vec
                .into_iter()
                .filter(|x| {
                    cursor_data
                        .as_ref()
                        .map(|cursor_data| x < cursor_data)
                        .unwrap_or(true)
                })
                .take(limit as usize)
                .collect();
        }
    };
    vec
}
