#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetProfileActorDto {
    pub profile_nanoid: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetProfileWebFingerDto {
    pub username: String,
    pub domain: String,
}
