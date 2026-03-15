use kernel::prelude::entity::{FieldAction, Profile};

#[derive(Debug)]
pub struct CreateProfileDto {
    pub account_nanoid: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
}

#[derive(Debug)]
pub struct UpdateProfileDto {
    pub account_nanoid: String,
    pub display_name: FieldAction<String>,
    pub summary: FieldAction<String>,
    pub icon_url: FieldAction<String>,
    pub banner_url: FieldAction<String>,
}

#[derive(Debug)]
pub struct ProfileDto {
    pub account_nanoid: String,
    pub nanoid: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
}

impl ProfileDto {
    pub fn new(
        profile: Profile,
        account_nanoid: String,
        icon_url: Option<String>,
        banner_url: Option<String>,
    ) -> Self {
        Self {
            account_nanoid,
            nanoid: profile.nanoid().as_ref().to_string(),
            display_name: profile
                .display_name()
                .as_ref()
                .map(|d| d.as_ref().to_string()),
            summary: profile.summary().as_ref().map(|s| s.as_ref().to_string()),
            icon_url,
            banner_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::test_utils::{ProfileBuilder, DEFAULT_DISPLAY_NAME, DEFAULT_SUMMARY};

    #[test]
    fn test_profile_dto_with_all_fields() {
        let profile = ProfileBuilder::new().build();
        let account_nanoid = "acc-nanoid-123".to_string();

        let icon_url = "https://example.com/icon.png".to_string();
        let banner_url = "https://example.com/banner.png".to_string();
        let nanoid_str = profile.nanoid().as_ref().to_string();
        let dto = ProfileDto::new(
            profile,
            account_nanoid.clone(),
            Some(icon_url.clone()),
            Some(banner_url.clone()),
        );

        assert_eq!(dto.account_nanoid, account_nanoid);
        assert_eq!(dto.nanoid, nanoid_str);
        assert_eq!(dto.display_name, Some(DEFAULT_DISPLAY_NAME.to_string()));
        assert_eq!(dto.summary, Some(DEFAULT_SUMMARY.to_string()));
        assert_eq!(dto.icon_url, Some(icon_url));
        assert_eq!(dto.banner_url, Some(banner_url));
    }

    #[test]
    fn test_profile_dto_with_no_optional_fields() {
        let profile = ProfileBuilder::new()
            .display_name(None::<String>)
            .summary(None::<String>)
            .build();
        let account_nanoid = "acc-nanoid-456".to_string();
        let nanoid_str = profile.nanoid().as_ref().to_string();

        let dto = ProfileDto::new(profile, account_nanoid.clone(), None, None);

        assert_eq!(dto.account_nanoid, account_nanoid);
        assert_eq!(dto.nanoid, nanoid_str);
        assert!(dto.display_name.is_none());
        assert!(dto.summary.is_none());
        assert!(dto.icon_url.is_none());
        assert!(dto.banner_url.is_none());
    }
}
