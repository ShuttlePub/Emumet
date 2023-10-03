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
    banner: Banner
}

impl Profile {
    pub fn new(
        id: impl Into<Id<Account>>,
        display_name: impl Into<DisplayName>,
        summary: impl Into<Summary>,
        icon: impl Into<Icon>,
        banner: impl Into<Banner>
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            summary: summary.into(),
            icon: icon.into(),
            banner: banner.into()
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
