use crate::entity::{
    AccountId, EventVersion, ImageId, Nanoid, Profile, ProfileDisplayName, ProfileId,
    ProfileSummary,
};

use super::{DEFAULT_DISPLAY_NAME, DEFAULT_SUMMARY};

pub struct ProfileBuilder {
    id: Option<ProfileId>,
    account_id: Option<AccountId>,
    display_name: Option<Option<ProfileDisplayName>>,
    summary: Option<Option<ProfileSummary>>,
    icon: Option<Option<ImageId>>,
    banner: Option<Option<ImageId>>,
    version: Option<EventVersion<Profile>>,
    nanoid: Option<Nanoid<Profile>>,
}

impl Default for ProfileBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfileBuilder {
    pub fn new() -> Self {
        Self {
            id: None,
            account_id: None,
            display_name: None,
            summary: None,
            icon: None,
            banner: None,
            version: None,
            nanoid: None,
        }
    }

    pub fn id(mut self, id: ProfileId) -> Self {
        self.id = Some(id);
        self
    }

    pub fn account_id(mut self, account_id: AccountId) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub fn display_name(mut self, display_name: Option<impl Into<String>>) -> Self {
        self.display_name = Some(display_name.map(|s| ProfileDisplayName::new(s)));
        self
    }

    pub fn summary(mut self, summary: Option<impl Into<String>>) -> Self {
        self.summary = Some(summary.map(|s| ProfileSummary::new(s)));
        self
    }

    pub fn icon(mut self, icon: Option<ImageId>) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn banner(mut self, banner: Option<ImageId>) -> Self {
        self.banner = Some(banner);
        self
    }

    pub fn version(mut self, version: EventVersion<Profile>) -> Self {
        self.version = Some(version);
        self
    }

    pub fn nanoid(mut self, nanoid: Nanoid<Profile>) -> Self {
        self.nanoid = Some(nanoid);
        self
    }

    pub fn build(self) -> Profile {
        crate::ensure_generator_initialized();
        Profile::new(
            self.id
                .unwrap_or_else(|| ProfileId::new(crate::generate_id())),
            self.account_id.unwrap_or_default(),
            self.display_name
                .unwrap_or_else(|| Some(ProfileDisplayName::new(DEFAULT_DISPLAY_NAME))),
            self.summary
                .unwrap_or_else(|| Some(ProfileSummary::new(DEFAULT_SUMMARY))),
            self.icon.unwrap_or(None),
            self.banner.unwrap_or(None),
            self.version.unwrap_or_default(),
            self.nanoid.unwrap_or_default(),
        )
    }
}
