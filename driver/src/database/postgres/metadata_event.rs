use error_stack::{Report, ResultExt};
use sqlx::PgConnection;
use time::OffsetDateTime;

use kernel::interfaces::modify::{DependOnMetadataEventModifier, MetadataEventModifier};
use kernel::interfaces::query::{
    DependOnMetadataEventQuery, MetadataEventQuery,
};
use kernel::prelude::entity::{
    CommandEnvelope, CreatedAt, EventEnvelope, EventVersion, ExpectedEventVersion, Metadata,
    MetadataEvent, MetadataId,
};
use kernel::KernelError;

use crate::database::postgres::{CountRow, VersionRow};
use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;

#[derive(sqlx::FromRow)]
struct MetadataEventRow {
    version: i64,
    metadata_id: i64,
    event_name: String,
    data: serde_json::Value,
    created_at: OffsetDateTime,
}

impl TryFrom<MetadataEventRow> for EventEnvelope<MetadataEvent, Metadata> {
    type Error = Report<KernelError>;

    fn try_from(value: MetadataEventRow) -> Result<Self, Self::Error> {
        let event: MetadataEvent = serde_json::from_value(value.data.clone()).convert_error()?;
        Ok(EventEnvelope::new(
            event,
            EventVersion::new(value.version),
            CreatedAt::new(value.created_at),
        ))
    }
}

pub struct PostgresMetadataEventRepository;

impl MetadataEventQuery for PostgresMetadataEventRepository {
    type Transaction = PostgresConnection;
    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
        since: Option<&EventVersion<Metadata>>,
    ) -> error_stack::Result<Vec<EventEnvelope<MetadataEvent, Metadata>>, KernelError> {
        let con: &mut PgConnection = transaction;
        if let Some(version) = since {
            sqlx::query_as::<_, MetadataEventRow>(
                // language=postgresql
                r#"
                SELECT version, metadata_id, event_name, data, created_at
                FROM metadata_events
                WHERE metadata_id = $2 AND version > $1
                ORDER BY version ASC
                "#,
            )
            .bind(version.as_ref())
        } else {
            sqlx::query_as::<_, MetadataEventRow>(
                // language=postgresql
                r#"
                SELECT version, metadata_id, event_name, data, created_at
                FROM metadata_events
                WHERE metadata_id = $1
                ORDER BY version ASC
                "#,
            )
        }
        .bind(metadata_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()
        .and_then(|versions| versions.into_iter().map(|event| event.try_into()).collect())
    }
}

impl DependOnMetadataEventQuery for PostgresDatabase {
    type MetadataEventQuery = PostgresMetadataEventRepository;

    fn metadata_event_query(&self) -> &Self::MetadataEventQuery {
        &PostgresMetadataEventRepository
    }
}

impl MetadataEventModifier for PostgresMetadataEventRepository {
    type Transaction = PostgresConnection;

    async fn handle(
        &self,
        transaction: &mut Self::Transaction,
        metadata_id: &MetadataId,
        event: &CommandEnvelope<MetadataEvent, Metadata>,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        let event_name = event.event().name();
        let version = event.version().as_ref();
        if let Some(version) = version {
            let version = match version {
                ExpectedEventVersion::Nothing => {
                    let amount = sqlx::query_as::<_, CountRow>(
                        // language=postgresql
                        r#"
                        SELECT COUNT(*)
                        FROM metadata_events
                        WHERE metadata_id = $1
                        "#,
                    )
                    .bind(metadata_id.as_ref())
                    .fetch_one(&mut *con)
                    .await
                    .convert_error()?;
                    if amount.count != 0 {
                        return Err(KernelError::Concurrency).attach_printable(format!(
                            "Metadata with id {} already exists",
                            metadata_id.as_ref()
                        ));
                    }
                    0
                }
                ExpectedEventVersion::Exact(version) => {
                    let last_version = sqlx::query_as::<_, VersionRow>(
                        // language=postgresql
                        r#"
                        SELECT version
                        FROM metadata_events
                        WHERE metadata_id = $1
                        ORDER BY version DESC
                        LIMIT 1
                        "#,
                    )
                    .bind(metadata_id.as_ref())
                    .fetch_one(&mut *con)
                    .await
                    .convert_error()?;
                    if last_version.version != *version.as_ref() {
                        return Err(KernelError::Concurrency).attach_printable(format!(
                            "Metadata with id {} version {} already exists",
                            metadata_id.as_ref(),
                            version.as_ref()
                        ));
                    }
                    *version.as_ref() + 1
                }
            };
            sqlx::query(
                // language=postgresql
                r#"
                INSERT INTO metadata_events (version, metadata_id, event_name, data, created_at)
                VALUES ($1, $2, $3, $4, now())
                "#,
            )
            .bind(version)
            .bind(metadata_id.as_ref())
            .bind(event_name)
            .bind(serde_json::to_value(event.event()).convert_error()?)
            .execute(con)
            .await
            .convert_error()?;
        } else {
            sqlx::query(
                // language=postgresql
                r#"
                INSERT INTO metadata_events (metadata_id, event_name, data, created_at)
                VALUES ($1, $2, $3, now())
                "#,
            )
            .bind(metadata_id.as_ref())
            .bind(event_name)
            .bind(serde_json::to_value(event.event()).convert_error()?)
            .execute(con)
            .await
            .convert_error()?;
        }
        Ok(())
    }
}

impl DependOnMetadataEventModifier for PostgresDatabase {
    type MetadataEventModifier = PostgresMetadataEventRepository;

    fn metadata_event_modifier(&self) -> &Self::MetadataEventModifier {
        &PostgresMetadataEventRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnMetadataEventModifier, MetadataEventModifier};
        use kernel::interfaces::query::{DependOnMetadataQuery};
        use kernel::prelude::entity::{
            AccountId, Metadata, MetadataContent, MetadataId, MetadataLabel,
        };

        use crate::database::postgres::PostgresDatabase;

        #[tokio::test]
        async fn find_by_id() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata_id = MetadataId::new(Uuid::new_v4());

            let events = db
                .metadata_query()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);

            let created_metadata = Metadata::create(
                account_id,
                MetadataLabel::new("label"),
                MetadataContent::new("content"),
            );
            let updated_metadata =
                Metadata::update(MetadataLabel::new("label"), MetadataContent::new("content"));
            let deleted_metadata = Metadata::delete();

            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &created_metadata)
                .await
                .unwrap();
            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &updated_metadata)
                .await
                .unwrap();
            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &deleted_metadata)
                .await
                .unwrap();

            let events = db
                .metadata_query()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[0].event(), created_metadata.event());
            assert_eq!(events[1].event(), updated_metadata.event());
            assert_eq!(events[2].event(), deleted_metadata.event());
        }

        #[tokio::test]
        async fn find_by_id_with_version() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata_id = MetadataId::new(Uuid::new_v4());

            let events = db
                .metadata_query()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap();
            assert_eq!(events.len(), 0);

            let created_metadata = Metadata::create(
                account_id,
                MetadataLabel::new("label"),
                MetadataContent::new("content"),
            );
            let updated_metadata =
                Metadata::update(MetadataLabel::new("label"), MetadataContent::new("content"));

            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &created_metadata)
                .await
                .unwrap();
            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &updated_metadata)
                .await
                .unwrap();

            let all_events = db
                .metadata_query()
                .find_by_id(&mut transaction, &metadata_id, None)
                .await
                .unwrap();
            let events = db
                .metadata_query()
                .find_by_id(
                    &mut transaction,
                    &metadata_id,
                    Some(all_events[1].version()),
                )
                .await
                .unwrap();
            assert_eq!(events.len(), 1);
            let event = events[0].event();
            assert_eq!(event, updated_metadata.event());
        }
    }

    mod modify {
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{DependOnMetadataEventModifier, MetadataEventModifier};
        use kernel::interfaces::query::DependOnMetadataQuery;
        use kernel::prelude::entity::{
            AccountId, Metadata, MetadataContent, MetadataId, MetadataLabel,
        };

        use crate::database::postgres::PostgresDatabase;

        #[tokio::test]
        async fn handle() {
            let db = PostgresDatabase::new().await.unwrap();
            let mut transaction = db.begin_transaction().await.unwrap();

            let account_id = AccountId::new(Uuid::new_v4());
            let metadata_id = MetadataId::new(Uuid::new_v4());

            let created_metadata = Metadata::create(
                account_id,
                MetadataLabel::new("label"),
                MetadataContent::new("content"),
            );
            let updated_metadata =
                Metadata::update(MetadataLabel::new("label"), MetadataContent::new("content"));
            let deleted_metadata = Metadata::delete();

            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &created_metadata)
                .await
                .unwrap();
            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &updated_metadata)
                .await
                .unwrap();
            db.metadata_event_modifier()
                .handle(&mut transaction, &metadata_id, &deleted_metadata)
                .await
                .unwrap();

            let events = db
                .metadata_query()
                .find_by_id(&mut transaction, &metadata_id)
                .await
                .unwrap();
            assert_eq!(events.len(), 3);
            assert_eq!(events[0].event(), created_metadata.event());
            assert_eq!(events[1].event(), updated_metadata.event());
            assert_eq!(events[2].event(), deleted_metadata.event());
        }
    }
}
