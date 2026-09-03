use kernel::interfaces::read_model::ProfileTransferRequestProjection;

#[derive(Debug)]
pub struct ProfileTransferRequestDto {
    pub nanoid: String,
    pub profile_nanoid: String,
    pub org_account_nanoid: String,
    pub status: String,
}

impl ProfileTransferRequestDto {
    pub fn new(
        projection: ProfileTransferRequestProjection,
        profile_nanoid: String,
        org_account_nanoid: String,
        status: String,
    ) -> Self {
        Self {
            nanoid: projection.nanoid().as_ref().to_string(),
            profile_nanoid,
            org_account_nanoid,
            status,
        }
    }
}
