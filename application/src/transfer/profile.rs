use kernel::prelude::entity::Profile;
use uuid::Uuid;

#[derive(Debug)]
pub struct ProfileDto {
    pub account_nanoid: String,
    pub nanoid: String,
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_id: Option<Uuid>,
    pub banner_id: Option<Uuid>,
}

impl ProfileDto {
    pub fn new(profile: Profile, account_nanoid: String) -> Self {
        Self {
            account_nanoid,
            nanoid: profile.nanoid().as_ref().to_string(),
            display_name: profile
                .display_name()
                .as_ref()
                .map(|d| d.as_ref().to_string()),
            summary: profile.summary().as_ref().map(|s| s.as_ref().to_string()),
            icon_id: profile.icon().as_ref().map(|i| *i.as_ref()),
            banner_id: profile.banner().as_ref().map(|b| *b.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::prelude::entity::{
        AccountId, EventVersion, ImageId, Nanoid, Profile, ProfileDisplayName, ProfileId,
        ProfileSummary,
    };
    use uuid::Uuid;

    #[test]
    fn test_profile_dto_with_all_fields() {
        let profile_id = ProfileId::new(Uuid::now_v7());
        let account_id = AccountId::new(Uuid::now_v7());
        let nanoid = Nanoid::default();
        let display_name = ProfileDisplayName::new("Test User".to_string());
        let summary = ProfileSummary::new("A test summary".to_string());
        let icon_id = ImageId::new(Uuid::now_v7());
        let banner_id = ImageId::new(Uuid::now_v7());
        let version = EventVersion::new(Uuid::now_v7());
        let account_nanoid = "acc-nanoid-123".to_string();

        let profile = Profile::new(
            profile_id,
            account_id,
            Some(display_name.clone()),
            Some(summary.clone()),
            Some(icon_id.clone()),
            Some(banner_id.clone()),
            version,
            nanoid.clone(),
        );

        let dto = ProfileDto::new(profile, account_nanoid.clone());

        assert_eq!(dto.account_nanoid, account_nanoid);
        assert_eq!(dto.nanoid, nanoid.as_ref().to_string());
        assert_eq!(dto.display_name, Some(display_name.as_ref().to_string()));
        assert_eq!(dto.summary, Some(summary.as_ref().to_string()));
        assert_eq!(dto.icon_id, Some(*icon_id.as_ref()));
        assert_eq!(dto.banner_id, Some(*banner_id.as_ref()));
    }

    #[test]
    fn test_profile_dto_with_no_optional_fields() {
        let profile_id = ProfileId::new(Uuid::now_v7());
        let account_id = AccountId::new(Uuid::now_v7());
        let nanoid = Nanoid::default();
        let version = EventVersion::new(Uuid::now_v7());
        let account_nanoid = "acc-nanoid-456".to_string();

        let profile = Profile::new(
            profile_id,
            account_id,
            None,
            None,
            None,
            None,
            version,
            nanoid.clone(),
        );

        let dto = ProfileDto::new(profile, account_nanoid.clone());

        assert_eq!(dto.account_nanoid, account_nanoid);
        assert_eq!(dto.nanoid, nanoid.as_ref().to_string());
        assert!(dto.display_name.is_none());
        assert!(dto.summary.is_none());
        assert!(dto.icon_id.is_none());
        assert!(dto.banner_id.is_none());
    }
}
