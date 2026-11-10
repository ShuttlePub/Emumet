use kernel::prelude::entity::{DomainControl, DomainControlMode};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DomainControlDto {
    pub profile_nanoid: String,
    pub domain: String,
    pub mode: DomainControlMode,
    pub revision: u64,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

impl DomainControlDto {
    pub fn new(profile_nanoid: &str, control: &DomainControl) -> Self {
        Self {
            profile_nanoid: profile_nanoid.to_string(),
            domain: control.domain().as_str().to_string(),
            mode: control.mode(),
            revision: control.revision(),
            created_at: control.created_at(),
            updated_at: control.updated_at(),
        }
    }
}
