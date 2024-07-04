mod blur_hash;
mod hash;
mod id;
mod url;

use destructure::Destructure;
use serde::{Deserialize, Serialize};
use vodca::References;

pub use self::blur_hash::*;
pub use self::hash::*;
pub use self::id::*;
pub use self::url::*;

#[derive(Debug, Clone, References, Destructure, Serialize, Deserialize)]
pub struct Image {
    id: ImageId,
    url: ImageUrl,
    hash: ImageHash,
    blur_hash: ImageBlurHash,
}

impl Image {
    pub fn new(id: ImageId, url: ImageUrl, hash: ImageHash, blur_hash: ImageBlurHash) -> Self {
        Self {
            id,
            url,
            hash,
            blur_hash,
        }
    }
}
