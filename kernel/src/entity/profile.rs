mod display_name;
mod id;
mod summary;
mod username;

pub use self::display_name::*;
pub use self::id::*;
pub use self::summary::*;
pub use self::username::*;

use super::{
    AccountId, CommandEnvelope, EventEnvelope, EventId, EventVersion, ExpectedVersion, FieldAction,
    Nanoid,
};
use crate::entity::image::ImageId;
use crate::event::EventApplier;
use crate::KernelError;
use destructure::Destructure;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{Nameln, Newln, References};

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Destructure, Serialize, Deserialize,
)]
pub struct Profile {
    id: ProfileId,
    account_id: AccountId,
    display_name: Option<ProfileDisplayName>,
    summary: Option<ProfileSummary>,
    icon: Option<ImageId>,
    banner: Option<ImageId>,
    version: EventVersion<Profile>,
    nanoid: Nanoid<Profile>,
    /// Set at most once (at creation or later via `UsernameSet`) and immutable afterwards.
    username: Option<ProfileUsername>,
}

#[derive(Debug, Clone, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[vodca(prefix = "profile", snake_case)]
pub enum ProfileEvent {
    Created {
        account_id: AccountId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
        nanoid: Nanoid<Profile>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        username: Option<ProfileUsername>,
    },
    /// One-time username assignment for a Profile that has none. Never emitted twice for a Profile
    /// (the event store enforces this with a unique index; `apply` rejects a second one).
    UsernameSet { username: ProfileUsername },
    Updated {
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        display_name: FieldAction<ProfileDisplayName>,
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        summary: FieldAction<ProfileSummary>,
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        icon: FieldAction<ImageId>,
        #[serde(default, skip_serializing_if = "FieldAction::is_unchanged")]
        banner: FieldAction<ImageId>,
    },
    AccountTransferred {
        from_account_id: AccountId,
        to_account_id: AccountId,
    },
}

impl Profile {
    pub fn create(
        id: ProfileId,
        account_id: AccountId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
        nano_id: Nanoid<Profile>,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        Self::create_with_username(
            id,
            account_id,
            display_name,
            summary,
            icon,
            banner,
            nano_id,
            None,
        )
    }

    /// Like [`Profile::create`], additionally setting the one-time username at creation.
    #[allow(clippy::too_many_arguments)]
    pub fn create_with_username(
        id: ProfileId,
        account_id: AccountId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
        nano_id: Nanoid<Profile>,
        username: Option<ProfileUsername>,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::Created {
            account_id,
            display_name,
            summary,
            icon,
            banner,
            nanoid: nano_id,
            username,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(ExpectedVersion::Nothing),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reconstitute(
        id: ProfileId,
        account_id: AccountId,
        display_name: Option<ProfileDisplayName>,
        summary: Option<ProfileSummary>,
        icon: Option<ImageId>,
        banner: Option<ImageId>,
        version: EventVersion<Profile>,
        nanoid: Nanoid<Profile>,
        username: Option<ProfileUsername>,
    ) -> Self {
        Self {
            id,
            account_id,
            display_name,
            summary,
            icon,
            banner,
            version,
            nanoid,
            username,
        }
    }

    /// One-time username assignment. No optimistic concurrency check: uniqueness and "at most
    /// once per Profile" are enforced by the event store's unique indexes.
    pub fn set_username(
        id: ProfileId,
        username: ProfileUsername,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::UsernameSet { username };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    /// Last-write-wins: no optimistic concurrency check.
    pub fn update(
        id: ProfileId,
        display_name: FieldAction<ProfileDisplayName>,
        summary: FieldAction<ProfileSummary>,
        icon: FieldAction<ImageId>,
        banner: FieldAction<ImageId>,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::Updated {
            display_name,
            summary,
            icon,
            banner,
        };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }

    pub fn transfer_account(
        id: ProfileId,
        from: AccountId,
        to: AccountId,
    ) -> CommandEnvelope<ProfileEvent, Profile> {
        let event = ProfileEvent::AccountTransferred {
            from_account_id: from,
            to_account_id: to,
        };
        CommandEnvelope::new(EventId::from(id), event.name(), event, None)
    }
}

impl EventApplier for Profile {
    type Event = ProfileEvent;
    const ENTITY_NAME: &'static str = "Profile";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError> {
        match event.event {
            ProfileEvent::Created {
                account_id,
                display_name,
                summary,
                icon,
                banner,
                nanoid: nano_id,
                username,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(Profile {
                    id: ProfileId::new(event.id),
                    account_id,
                    display_name,
                    summary,
                    icon,
                    banner,
                    version: event.version,
                    nanoid: nano_id,
                    username,
                });
            }
            ProfileEvent::UsernameSet { username } => {
                if let Some(profile) = entity {
                    if profile.username.is_some() {
                        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                            "Profile {} already has a username; it is set only once",
                            event.id.as_ref()
                        )));
                    }
                    profile.username = Some(username);
                    profile.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            ProfileEvent::Updated {
                display_name,
                summary,
                icon,
                banner,
            } => {
                if let Some(profile) = entity {
                    match display_name {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.display_name = None,
                        FieldAction::Set(v) => profile.display_name = Some(v),
                    }
                    match summary {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.summary = None,
                        FieldAction::Set(v) => profile.summary = Some(v),
                    }
                    match icon {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.icon = None,
                        FieldAction::Set(v) => profile.icon = Some(v),
                    }
                    match banner {
                        FieldAction::Unchanged => {}
                        FieldAction::Clear => profile.banner = None,
                        FieldAction::Set(v) => profile.banner = Some(v),
                    }
                    profile.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
            ProfileEvent::AccountTransferred {
                from_account_id,
                to_account_id,
            } => {
                if let Some(profile) = entity {
                    if profile.account_id != from_account_id {
                        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                            "Profile {} is not owned by account {}",
                            event.id.as_ref(),
                            from_account_id.as_ref()
                        )));
                    }
                    profile.account_id = to_account_id;
                    profile.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::entity::{
        AccountId, EventEnvelope, EventVersion, FieldAction, ImageId, Nanoid, Profile,
        ProfileDisplayName, ProfileEvent, ProfileId, ProfileSummary, ProfileUsername,
    };
    use crate::event::EventApplier;
    use crate::test_utils::ProfileBuilder;
    use crate::KernelError;

    #[test]
    fn create_profile() {
        crate::ensure_generator_initialized();
        let account_id = AccountId::default();
        let id = ProfileId::new(crate::generate_id());
        let nano_id = Nanoid::default();
        let create_event = Profile::create(
            id.clone(),
            account_id.clone(),
            None,
            None,
            None,
            None,
            nano_id.clone(),
        );
        let envelope = EventEnvelope::new(
            create_event.id().clone(),
            create_event.event().clone(),
            EventVersion::default(),
        );
        let mut profile = None;
        Profile::apply(&mut profile, envelope).unwrap();
        assert!(profile.is_some());
        let profile = profile.unwrap();
        assert_eq!(profile.id(), &id);
        assert_eq!(profile.account_id(), &account_id);
        assert!(profile.display_name().is_none());
        assert!(profile.summary().is_none());
        assert!(profile.icon().is_none());
        assert!(profile.banner().is_none());
        assert_eq!(profile.nanoid(), &nano_id);
    }

    #[test]
    fn update_profile() {
        crate::ensure_generator_initialized();
        let account_id = AccountId::default();
        let id = ProfileId::new(crate::generate_id());
        let nano_id = Nanoid::default();
        let profile = ProfileBuilder::new()
            .id(id.clone())
            .account_id(account_id.clone())
            .display_name(None::<String>)
            .summary(None::<String>)
            .nanoid(nano_id.clone())
            .build();
        let display_name = ProfileDisplayName::new("display_name".to_string());
        let summary = ProfileSummary::new("summary".to_string());
        let icon = ImageId::new(crate::generate_id());
        let banner = ImageId::new(crate::generate_id());
        let update_event = Profile::update(
            id.clone(),
            FieldAction::Set(display_name.clone()),
            FieldAction::Set(summary.clone()),
            FieldAction::Set(icon.clone()),
            FieldAction::Set(banner.clone()),
        );
        let version = EventVersion::default();
        let envelope = EventEnvelope::new(
            update_event.id().clone(),
            update_event.event().clone(),
            version.clone(),
        );
        let mut profile = Some(profile);
        Profile::apply(&mut profile, envelope).unwrap();
        assert!(profile.is_some());
        let profile = profile.unwrap();
        assert_eq!(profile.id(), &id);
        assert_eq!(profile.account_id(), &account_id);
        assert_eq!(profile.display_name().as_ref().unwrap(), &display_name);
        assert_eq!(profile.summary().as_ref().unwrap(), &summary);
        assert_eq!(profile.icon().as_ref().unwrap(), &icon);
        assert_eq!(profile.banner().as_ref().unwrap(), &banner);
        assert_eq!(profile.version(), &version);
        assert_eq!(profile.nanoid(), &nano_id);
    }

    fn envelope(
        event: crate::entity::CommandEnvelope<ProfileEvent, Profile>,
        version: i64,
    ) -> EventEnvelope<ProfileEvent, Profile> {
        EventEnvelope::new(
            event.id().clone(),
            event.event().clone(),
            EventVersion::new(version),
        )
    }

    #[test]
    fn create_with_username_sets_it_and_plain_create_leaves_it_unset() {
        crate::ensure_generator_initialized();
        let id = ProfileId::new(crate::generate_id());
        let mut profile = None;
        Profile::apply(
            &mut profile,
            envelope(
                Profile::create_with_username(
                    id.clone(),
                    AccountId::default(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                    Some(ProfileUsername::new("alice")),
                ),
                1,
            ),
        )
        .unwrap();
        assert_eq!(
            profile.unwrap().username(),
            &Some(ProfileUsername::new("alice"))
        );

        let mut plain = None;
        Profile::apply(
            &mut plain,
            envelope(
                Profile::create(
                    ProfileId::new(crate::generate_id()),
                    AccountId::default(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                ),
                1,
            ),
        )
        .unwrap();
        assert!(plain.unwrap().username().is_none());
    }

    #[test]
    fn username_is_set_once_and_survives_other_events() {
        crate::ensure_generator_initialized();
        let profile = ProfileBuilder::new().build();
        let id = profile.id().clone();
        let mut profile = Some(profile);
        Profile::apply(
            &mut profile,
            envelope(
                Profile::set_username(id.clone(), ProfileUsername::new("alice")),
                2,
            ),
        )
        .unwrap();
        assert_eq!(
            profile.as_ref().unwrap().username(),
            &Some(ProfileUsername::new("alice"))
        );
        assert_eq!(profile.as_ref().unwrap().version(), &EventVersion::new(2));

        // A second assignment (whatever the value) is rejected and changes nothing.
        let error = Profile::apply(
            &mut profile,
            envelope(
                Profile::set_username(id.clone(), ProfileUsername::new("bob")),
                3,
            ),
        )
        .unwrap_err();
        assert_eq!(error.current_context(), &KernelError::Rejected);
        assert_eq!(
            profile.as_ref().unwrap().username(),
            &Some(ProfileUsername::new("alice"))
        );

        // Unrelated updates and transfers keep the username.
        Profile::apply(
            &mut profile,
            envelope(
                Profile::update(
                    id.clone(),
                    FieldAction::Set(ProfileDisplayName::new("name".to_string())),
                    FieldAction::Unchanged,
                    FieldAction::Unchanged,
                    FieldAction::Unchanged,
                ),
                4,
            ),
        )
        .unwrap();
        let from = profile.as_ref().unwrap().account_id().clone();
        Profile::apply(
            &mut profile,
            envelope(Profile::transfer_account(id, from, AccountId::default()), 5),
        )
        .unwrap();
        assert_eq!(
            profile.unwrap().username(),
            &Some(ProfileUsername::new("alice"))
        );
    }

    #[test]
    fn username_set_on_a_missing_profile_is_an_error() {
        crate::ensure_generator_initialized();
        let id = ProfileId::new(crate::generate_id());
        let mut profile = None;
        assert!(Profile::apply(
            &mut profile,
            envelope(Profile::set_username(id, ProfileUsername::new("alice")), 1),
        )
        .is_err());
    }

    #[test]
    fn created_event_serialization_is_backward_compatible() {
        // Events stored before P1 have no `username` key and must still deserialize.
        let legacy = serde_json::json!({
            "type": "created",
            "account_id": 1,
            "display_name": null,
            "summary": null,
            "icon": null,
            "banner": null,
            "nanoid": "legacy",
        });
        let event: ProfileEvent = serde_json::from_value(legacy).unwrap();
        assert!(matches!(
            event,
            ProfileEvent::Created { username: None, .. }
        ));

        // Without a username the stored JSON is unchanged (no `username` key at all), so the
        // event-table unique indexes only ever see real usernames.
        let json = serde_json::to_value(&event).unwrap();
        assert!(json.get("username").is_none());

        let with = ProfileEvent::UsernameSet {
            username: ProfileUsername::new("alice"),
        };
        let json = serde_json::to_value(&with).unwrap();
        assert_eq!(json["type"], "username_set");
        assert_eq!(json["username"], "alice");
        assert_eq!(serde_json::from_value::<ProfileEvent>(json).unwrap(), with);
    }

    #[test]
    fn clear_icon_and_banner() {
        crate::ensure_generator_initialized();
        let id = ProfileId::new(crate::generate_id());
        let icon = ImageId::new(crate::generate_id());
        let banner = ImageId::new(crate::generate_id());
        let profile = ProfileBuilder::new()
            .id(id.clone())
            .display_name(None::<String>)
            .summary(None::<String>)
            .icon(Some(icon))
            .banner(Some(banner))
            .build();

        let update_event = Profile::update(
            id,
            FieldAction::Unchanged,
            FieldAction::Unchanged,
            FieldAction::Clear,
            FieldAction::Clear,
        );
        let new_version = EventVersion::default();
        let envelope = EventEnvelope::new(
            update_event.id().clone(),
            update_event.event().clone(),
            new_version,
        );
        let mut profile = Some(profile);
        Profile::apply(&mut profile, envelope).unwrap();
        let profile = profile.unwrap();
        assert!(profile.icon().is_none());
        assert!(profile.banner().is_none());
    }
}
