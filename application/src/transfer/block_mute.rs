pub struct BlockAccountDto {
    pub account_nanoid: String,
    pub target: String,
}

pub type MuteAccountDto = BlockAccountDto;

pub struct RelationDto {
    pub id: String,
    pub target_type: String,
    pub target: String,
}
