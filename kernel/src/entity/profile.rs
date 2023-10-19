mod banner;
mod display_name;
mod icon;
mod summary;

pub use self::banner::*;
pub use self::display_name::*;
pub use self::icon::*;
pub use self::summary::*;
use serde::{Deserialize, Serialize};
use vodca::References;

use super::AccountId;

#[derive(Debug, Clone, Hash, References, Serialize, Deserialize)]
pub struct Profile {
    id: AccountId,
    display_name: ProfileDisplayName,
    summary: ProfileSummary,
    icon: ProfileIcon,
    banner: ProfileBanner,
}

impl Profile {
    pub fn new(
        id: AccountId,
        display_name: ProfileDisplayName,
        summary: ProfileSummary,
        icon: ProfileIcon,
        banner: ProfileBanner,
    ) -> Self {
        Self {
            id,
            display_name,
            summary,
            icon,
            banner,
        }
    }
}
