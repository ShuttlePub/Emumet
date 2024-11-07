use vodca::Nameln;

pub mod account;

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
