#[derive(Debug, Clone)]
pub struct PublicBaseUrl(pub String);

impl PublicBaseUrl {
    pub fn new(url: String) -> Self {
        Self(url)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub trait DependOnPublicBaseUrl: Send + Sync {
    fn public_base_url(&self) -> &PublicBaseUrl;
}
