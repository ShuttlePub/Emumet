mod blurhash;
mod hash;
mod id;
mod url;

use destructure::Destructure;
use serde::{Deserialize, Serialize};
use vodca::{Newln, References};

pub use self::blurhash::*;
pub use self::hash::*;
pub use self::id::*;
pub use self::url::*;

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Destructure, Serialize, Deserialize,
)]
pub struct Image {
    id: ImageId,
    url: ImageUrl,
    hash: ImageHash,
    blur_hash: ImageBlurHash,
}
