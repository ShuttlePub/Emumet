use error_stack::Report;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use kernel::interfaces::modify::{DependOnProfileEventModifier, ProfileEventModifier};
use kernel::interfaces::query::{DependOnProfileEventQuery, ProfileEventQuery};
use kernel::prelude::entity::{
    Account, AccountId, CommandEnvelope, CreatedAt, EventEnvelope, EventVersion,
    ExpectedEventVersion, Profile, ProfileEvent,
};
use kernel::KernelError;

use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;

#[derive(sqlx::FromRow)]
struct ProfileEventRow {
    version: i64,
    account_id: Uuid,
    event_name: String,
    data: serde_json::Value,
    created_at: OffsetDateTime,
}

impl TryFrom<ProfileEventRow> for EventEnvelope<ProfileEvent, Profile> {
    type Error = Report<KernelError>;

    fn try_from(row: ProfileEventRow) -> Result<Self, Self::Error> {
        let event: ProfileEvent = serde_json::from_value(row.data).convert_error()?;
        Ok(EventEnvelope::new(
            event,
            EventVersion::new(row.version),
            CreatedAt::new(row.created_at),
        ))
    }
}

struct PostgresProfileEventRepository;

impl ProfileEventQuery for PostgresProfileEventRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &AccountId,
        since: Option<&EventVersion<Profile>>,
    ) -> error_stack::Result<Vec<EventEnvelope<ProfileEvent, Profile>>, KernelError> {
        let mut con: &PgConnection = transaction;
        if let Some(since) = since {
            sqlx::query_as::<_, ProfileEventRow>(
                //language=postgresql
                r#"
                SELECT version, account_id, event_name, data, created_at
                FROM profile_events
                WHERE account_id = $2 AND version > $1
                ORDER BY version
                "#,
            )
            .bind(since.as_ref())
        } else {
            sqlx::query_as::<_, ProfileEventRow>(
                //language=postgresql
                r#"
                SELECT version, account_id, event_name, data, created_at
                FROM profile_events
                WHERE account_id = $1
                ORDER BY version
                "#,
            )
        }
        .bind(id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(|row| row.try_into()).collect())
    }
}

impl DependOnProfileEventQuery for PostgresDatabase {
    type ProfileEventQuery = PostgresProfileEventRepository;

    fn profile_event_query(&self) -> &Self::ProfileEventQuery {
        &PostgresProfileEventRepository
    }
}

impl ProfileEventModifier for PostgresProfileEventRepository {
    type Transaction = PostgresConnection;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        account_id: &AccountId,
        event: &CommandEnvelope<ProfileEvent, Profile>,
    ) -> error_stack::Result<(), KernelError> {
        let mut con: &PgConnection = transaction;

        let event_name = event.event().name();
        let version = event.version().as_ref();
        if let Some(version) = version {
            match version {
                ExpectedEventVersion::Nothing => {
                    let amount = sqlx::query_as::<_, CountRow>(
                        //language=postgresql
                        r#"
                        SELECT COUNT(*)
                        FROM profile_events
                        WHERE account_id = $1
                        "#,
                    )
                    .bind(account_id.as_ref())
                    .fetch_one(con)
                    .await
                    .convert_error()?;
                    if amount.count > 0 {
                        return Err(Report::new(KernelError::Concurrency)
                            .attach_printable(format!("Profile {} already exists", account_id)));
                    }
                }
                ExpectedEventVersion::Exact(version) => {
                    let last_version = sqlx::query_as::<_, VersionRow>(
                        // language=postgresql
                        r#"
                        SELECT version
                        FROM profile_events
                        WHERE account_id = $1
                        ORDER BY version DESC
                        LIMIT 1
                        "#,
                    )
                    .bind(account_id.as_ref())
                    .fetch_optional(con)
                    .await
                    .convert_error()?;
                    if last_version
                        .map(|row| row.version != *version.as_ref())
                        .unwrap_or(true)
                    {
                        return Err(Report::new(KernelError::Concurrency).attach_printable(
                            format!(
                                "Profile {} version {} already exists",
                                account_id,
                                version.as_ref()
                            ),
                        ));
                    }
                }
            }
            sqlx::query(
                //language=postgresql
                r#"
            INSERT INTO profile_events (version, account_id, event_name, data, created_at)
            VALUES ($1, $2, $3, $4, now())
            "#,
            )
            .bind(version.as_ref())
            .bind(account_id.as_ref())
            .bind(event_name)
            .bind(serde_json::to_value(event.event()).convert_error()?)
            .execute(con)
            .await
            .convert_error()
        } else {
            sqlx::query(
                //language=postgresql
                r#"
            INSERT INTO profile_events (account_id, event_name, data, created_at)
            VALUES ($1, $2, $3, now())
            "#,
            )
            .bind(account_id.as_ref())
            .bind(event.event_name())
            .bind(serde_json::to_value(event.data()).convert_error()?)
            .execute(con)
            .await
            .convert_error()
        }
    }
}

impl DependOnProfileEventModifier for PostgresDatabase {
    type ProfileEventModifier = PostgresProfileEventRepository;

    fn profile_event_modifier(&self) -> &Self::ProfileEventModifier {
        &PostgresProfileEventRepository
    }
}

impl PostgresProfileEventRepository {
    // Used in the test
    async fn delete(
        &self,
        transaction: &PostgresConnection,
        account_id: &AccountId,
        event: &EventVersion<Account>,
    ) -> error_stack::Result<(), KernelError> {
        let mut con: &PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM profile_events
            WHERE account_id = $1 AND version = $2
            "#,
        )
        .bind(account_id.as_ref())
        .bind(event.as_ref())
        .execute(con)
        .await
        .convert_error()
    }
}

#[cfg(test)]
mod test {
    mod query {
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnProfileEventModifier, ProfileEventModifier};
        use kernel::interfaces::query::{DependOnProfileEventQuery, ProfileEventQuery};
        use kernel::prelude::entity::{AccountId, Profile, ProfileDisplayName, ProfileSummary};

        use crate::database::PostgresDatabase;
        use crate::ConvertError;

        #[tokio::test]
        async fn find_by_id() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut con = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::new_v4());
            let events = db
                .profile_event_query()
                .find_by_id(&mut con, &account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);
            let created_profile = Profile::create(
                Some(ProfileDisplayName::new("test")),
                Some(ProfileSummary::new("test")),
                None,
                None,
            );
            let updated_profile = Profile::update(
                Some(ProfileDisplayName::new("updated")),
                Some(ProfileSummary::new("updated")),
                None,
                None,
            );
            let deleted_profile = Profile::delete();

            db.profile_event_modifier()
                .handle(&mut con, &account_id, &created_profile.into())
                .await
                .unwrap();
            db.profile_event_modifier()
                .handle(&mut con, &account_id, &updated_profile.into())
                .await
                .unwrap();
            db.profile_event_modifier()
                .handle(&mut con, &account_id, &deleted_profile.into())
                .await
                .unwrap();

            let events = db
                .profile_event_query()
                .find_by_id(&mut con, &account_id, None)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[0].event(), created_profile.event());
            assert_eq!(events[1].event(), updated_profile.event());
            assert_eq!(events[2].event(), deleted_profile.event());
        }

        #[tokio::test]
        #[should_panic]
        async fn find_by_id_with_version() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut con = db.begin_transaction().await.unwrap();
            let account_id = AccountId::new(Uuid::new_v4());
            let created_profile = Profile::create(
                Some(ProfileDisplayName::new("test")),
                Some(ProfileSummary::new("test")),
                None,
                None,
            );
            let updated_profile = Profile::update(
                Some(ProfileDisplayName::new("updated")),
                Some(ProfileSummary::new("updated")),
                None,
                None,
            );
            db.profile_event_modifier()
                .handle(&mut con, &account_id, &created_profile.into())
                .await
                .unwrap();
            db.profile_event_modifier()
                .handle(&mut con, &account_id, &updated_profile.into())
                .await
                .unwrap();

            let all_events = db
                .profile_event_query()
                .find_by_id(&mut con, &account_id, None)
                .await
                .unwrap();

            let events = db
                .profile_event_query()
                .find_by_id(&mut con, &account_id, Some(all_events[1].version()))
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].event(), updated_profile.event());
        }

        mod modify {
            use uuid::Uuid;

            use kernel::interfaces::database::DatabaseConnection;
            use kernel::interfaces::modify::{DependOnProfileEventModifier, ProfileEventModifier};
            use kernel::interfaces::query::{DependOnProfileEventQuery, ProfileEventQuery};
            use kernel::prelude::entity::{AccountId, Profile, ProfileDisplayName, ProfileSummary};

            use crate::database::PostgresDatabase;

            #[tokio::test]
            async fn basic_creation() {
                let db = PostgresDatabase::new().await.unwrap();
                let mut transaction = db.begin_transaction().await.unwrap();
                let account_id = AccountId::new(Uuid::new_v4());
                let created_profile = Profile::create(
                    Some(ProfileDisplayName::new("test")),
                    Some(ProfileSummary::new("test")),
                    None,
                    None,
                );
                db.profile_event_modifier()
                    .handle(&mut transaction, &account_id, &created_profile.into())
                    .await
                    .unwrap();
                let events = db
                    .profile_event_query()
                    .find_by_id(&mut transaction, &account_id, None)
                    .await
                    .unwrap();
                assert_eq!(events.len(), 1);
                assert_eq!(events[0].event(), created_profile.event());
            }
        }
    }
}
