mod banner;
mod display_name;
mod icon;
mod summary;

pub use self::banner::*;
pub use self::display_name::*;
pub use self::icon::*;
pub use self::summary::*;

use super::Account;
use super::Id;

pub struct Profile {
    id: Id<Account>,
    display_name: DisplayName,
    summary: Summary,
    icon: Icon,
    banner: Banner,
}

impl Profile {
    pub fn new(
        id: Id<Account>,
        display_name: DisplayName,
        summary: Summary,
        icon: Icon,
        banner: Banner,
    ) -> Self {
        Self {
            id,
            display_name,
            summary,
            icon,
            banner,
        }
    }

    pub fn id(&self) -> &Id<Account> {
        &self.id
    }

    pub fn display_name(&self) -> &DisplayName {
        &self.display_name
    }

    pub fn summary(&self) -> &Summary {
        &self.summary
    }

    pub fn icon(&self) -> &Icon {
        &self.icon
    }

    pub fn banner(&self) -> &Banner {
        &self.banner
    }
}
