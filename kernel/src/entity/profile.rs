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
        id: impl Into<i64>,
        display_name: impl Into<String>,
        summary: impl Into<String>,
        icon: impl Into<String>,
        banner: impl Into<String>,
    ) -> Self {
        Self {
            id: Id::new(id),
            display_name: DisplayName::new(display_name),
            summary: Summary::new(summary),
            icon: Icon::new(icon),
            banner: Banner::new(banner),
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
