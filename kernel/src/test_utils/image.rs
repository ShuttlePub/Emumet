use crate::entity::{Image, ImageBlurHash, ImageHash, ImageId, ImageUrl};

use super::{unique_image_url, DEFAULT_BLUR_HASH, DEFAULT_IMAGE_HASH};

pub struct ImageBuilder {
    id: Option<ImageId>,
    url: Option<ImageUrl>,
    hash: Option<ImageHash>,
    blur_hash: Option<ImageBlurHash>,
}

impl Default for ImageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            url: None,
            hash: None,
            blur_hash: None,
        }
    }

    pub fn id(mut self, id: ImageId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(ImageUrl::new(url));
        self
    }

    pub fn hash(mut self, hash: impl Into<String>) -> Self {
        self.hash = Some(ImageHash::new(hash));
        self
    }

    pub fn blur_hash(mut self, blur_hash: impl Into<String>) -> Self {
        self.blur_hash = Some(ImageBlurHash::new(blur_hash));
        self
    }

    pub fn build(self) -> Image {
        crate::ensure_generator_initialized();
        Image::new(
            self.id
                .unwrap_or_else(|| ImageId::new(crate::generate_id())),
            self.url.unwrap_or_else(unique_image_url),
            self.hash
                .unwrap_or_else(|| ImageHash::new(DEFAULT_IMAGE_HASH)),
            self.blur_hash
                .unwrap_or_else(|| ImageBlurHash::new(DEFAULT_BLUR_HASH)),
        )
    }
}
