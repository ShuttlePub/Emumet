use time::OffsetDateTime;

pub struct AccountDto {
    pub nanoid: String,
    pub name: String,
    pub public_key: String,
    pub is_bot: bool,
    pub created_at: OffsetDateTime,
}
