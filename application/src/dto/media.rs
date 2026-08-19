#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadImageDto {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedImageDto {
    pub id: String,
    pub url: String,
    pub hash: String,
    pub blur_hash: String,
}
