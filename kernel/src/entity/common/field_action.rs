use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Hash, Eq, PartialEq, Default)]
pub enum FieldAction<T> {
    #[default]
    Unchanged,
    Clear,
    Set(T),
}

impl<T> FieldAction<T> {
    pub fn is_unchanged(&self) -> bool {
        matches!(self, FieldAction::Unchanged)
    }

    #[must_use]
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> FieldAction<U> {
        match self {
            FieldAction::Unchanged => FieldAction::Unchanged,
            FieldAction::Clear => FieldAction::Clear,
            FieldAction::Set(v) => FieldAction::Set(f(v)),
        }
    }
}

impl<T: Serialize> Serialize for FieldAction<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let opt: Option<Option<&T>> = match self {
            FieldAction::Unchanged => None,
            FieldAction::Clear => Some(None),
            FieldAction::Set(v) => Some(Some(v)),
        };
        opt.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for FieldAction<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let opt: Option<Option<T>> = Option::deserialize(deserializer)?;
        Ok(match opt {
            None => FieldAction::Unchanged,
            Some(None) => FieldAction::Clear,
            Some(Some(v)) => FieldAction::Set(v),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_transforms_value() {
        let action = FieldAction::Set(42);
        let mapped = action.map(|v| v.to_string());
        assert_eq!(mapped, FieldAction::Set("42".to_string()));

        let action: FieldAction<i32> = FieldAction::Clear;
        let mapped = action.map(|v| v.to_string());
        assert_eq!(mapped, FieldAction::<String>::Clear);

        let action: FieldAction<i32> = FieldAction::Unchanged;
        let mapped = action.map(|v| v.to_string());
        assert_eq!(mapped, FieldAction::<String>::Unchanged);
    }
}
