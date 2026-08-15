use super::profile_event_store::PostgresProfileEventStore;
use crate::database::{PostgresConnection, PostgresDatabase};
use error_stack::Report;
use kernel::interfaces::event_store::ProfileEventStore;
use kernel::interfaces::repository::{AggregateRepository, DependOnProfileRepository, Rehydrated};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, Profile, ProfileEvent, ProfileId,
};
use kernel::KernelError;

pub struct PostgresProfileRepository;

impl AggregateRepository<Profile> for PostgresProfileRepository {
    type Connection = PostgresConnection;
    type Id = ProfileId;

    async fn load(
        &self,
        executor: &mut Self::Connection,
        id: &Self::Id,
    ) -> error_stack::Result<Rehydrated<Profile>, KernelError> {
        let events = PostgresProfileEventStore
            .find_by_id(executor, &EventId::from(id.clone()), None)
            .await?;
        Rehydrated::<Profile>::from_events(events)?.ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable(format!("No events found for profile: {}", id.as_ref()))
        })
    }

    async fn save(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<ProfileEvent, Profile>,
    ) -> error_stack::Result<EventEnvelope<ProfileEvent, Profile>, KernelError> {
        PostgresProfileEventStore
            .persist_and_transform(executor, command)
            .await
    }
}

impl DependOnProfileRepository for PostgresDatabase {
    type ProfileRepository = PostgresProfileRepository;

    fn profile_repository(&self) -> &Self::ProfileRepository {
        &PostgresProfileRepository
    }
}

#[cfg(test)]
mod test {
    use super::PostgresProfileRepository;
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event_store::{DependOnProfileEventStore, ProfileEventStore};
    use kernel::interfaces::repository::AggregateRepository;
    use kernel::prelude::entity::{
        EventEnvelope, FieldAction, Profile, ProfileDisplayName, ProfileEvent, ProfileId,
        ProfileSummary,
    };
    use kernel::test_utils::ProfileBuilder;

    async fn create_profile(
        db: &PostgresDatabase,
    ) -> (ProfileId, EventEnvelope<ProfileEvent, Profile>) {
        let mut conn = db.connection().await.unwrap();
        let profile = ProfileBuilder::new().build();
        let id = profile.id().clone();
        let command = Profile::create(
            id.clone(),
            profile.account_id().clone(),
            profile.display_name().clone(),
            profile.summary().clone(),
            profile.icon().clone(),
            profile.banner().clone(),
            profile.nanoid().clone(),
        );
        let envelope = db
            .profile_event_store()
            .persist_and_transform(&mut conn, command)
            .await
            .unwrap();
        (id, envelope)
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn can_load_rehydrated_profile() {
        let db = PostgresDatabase::new().await.unwrap();
        let (id, _) = create_profile(&db).await;
        let mut conn = db.connection().await.unwrap();

        let loaded = PostgresProfileRepository
            .load(&mut conn, &id)
            .await
            .unwrap();
        assert_eq!(loaded.aggregate().id(), &id);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn can_save_and_update_profile() {
        let db = PostgresDatabase::new().await.unwrap();
        let (id, _) = create_profile(&db).await;
        let mut conn = db.connection().await.unwrap();
        let _initial = PostgresProfileRepository
            .load(&mut conn, &id)
            .await
            .unwrap();

        let update_command = Profile::update(
            id.clone(),
            FieldAction::Set(ProfileDisplayName::new("updated".to_string())),
            FieldAction::Set(ProfileSummary::new("updated summary".to_string())),
            FieldAction::Unchanged,
            FieldAction::Unchanged,
        );
        let envelope = PostgresProfileRepository
            .save(&mut conn, update_command)
            .await
            .unwrap();

        let updated = PostgresProfileRepository
            .load(&mut conn, &id)
            .await
            .unwrap();
        assert_eq!(
            updated.aggregate().display_name(),
            &Some(ProfileDisplayName::new("updated".to_string()))
        );
        assert_eq!(updated.version(), &envelope.version);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn save_without_expected_version_succeeds() {
        let db = PostgresDatabase::new().await.unwrap();
        let (id, _) = create_profile(&db).await;
        let mut conn = db.connection().await.unwrap();

        let update = Profile::update(
            id.clone(),
            FieldAction::Set(ProfileDisplayName::new("second".to_string())),
            FieldAction::Unchanged,
            FieldAction::Unchanged,
            FieldAction::Unchanged,
        );
        PostgresProfileRepository
            .save(&mut conn, update)
            .await
            .unwrap();

        let reloaded = PostgresProfileRepository
            .load(&mut conn, &id)
            .await
            .unwrap();
        assert_eq!(
            reloaded.aggregate().display_name(),
            &Some(ProfileDisplayName::new("second".to_string()))
        );
    }
}
