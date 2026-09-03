use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::event_store::{
    DependOnProfileTransferRequestEventStore, ProfileTransferRequestEventStore,
};
use kernel::prelude::entity::{
    CommandEnvelope, EventEnvelope, EventId, EventVersion, ExpectedVersion, ProfileTransferRequest,
    ProfileTransferRequestEvent,
};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct EventRow {
    version: i64,
    id: i64,
    #[allow(dead_code)]
    event_name: String,
    data: serde_json::Value,
}

impl TryFrom<EventRow> for EventEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest> {
    type Error = Report<KernelError>;

    fn try_from(value: EventRow) -> Result<Self, Self::Error> {
        let event: ProfileTransferRequestEvent =
            serde_json::from_value(value.data).convert_error()?;
        Ok(EventEnvelope::new(
            EventId::new(value.id),
            event,
            EventVersion::new(value.version),
        ))
    }
}

pub struct PostgresProfileTransferRequestEventStore;

impl ProfileTransferRequestEventStore for PostgresProfileTransferRequestEventStore {
    type Connection = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &EventId<ProfileTransferRequestEvent, ProfileTransferRequest>,
        since: Option<&EventVersion<ProfileTransferRequest>>,
    ) -> error_stack::Result<
        Vec<EventEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>>,
        KernelError,
    > {
        let con: &mut PgConnection = executor;
        let rows = if let Some(version) = since {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM profile_transfer_request_events
                WHERE id = $1 AND version > $2
                ORDER BY version
                "#,
            )
            .bind(id.as_ref())
            .bind(version.as_ref())
            .fetch_all(con)
            .await
            .convert_error()?
        } else {
            sqlx::query_as::<_, EventRow>(
                //language=postgresql
                r#"
                SELECT version, id, event_name, data
                FROM profile_transfer_request_events
                WHERE id = $1
                ORDER BY version
                "#,
            )
            .bind(id.as_ref())
            .fetch_all(con)
            .await
            .convert_error()?
        };
        rows.into_iter()
            .map(TryFrom::try_from)
            .collect::<error_stack::Result<Vec<_>, KernelError>>()
    }

    async fn persist(
        &self,
        executor: &mut Self::Connection,
        command: &CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
    ) -> error_stack::Result<(), KernelError> {
        self.persist_internal(executor, command, kernel::generate_id())
            .await
    }

    async fn persist_and_transform(
        &self,
        executor: &mut Self::Connection,
        command: CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
    ) -> error_stack::Result<
        EventEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
        KernelError,
    > {
        let version = kernel::generate_id();
        self.persist_internal(executor, &command, version).await?;

        let command = command.into_destruct();
        Ok(EventEnvelope::new(
            command.id,
            command.event,
            EventVersion::new(version),
        ))
    }
}

impl PostgresProfileTransferRequestEventStore {
    async fn persist_internal(
        &self,
        executor: &mut PostgresConnection,
        command: &CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest>,
        version: i64,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let event_name = command.event_name();
        let data = serde_json::to_value(command.event()).convert_error()?;
        let prev_version = command.prev_version().as_ref();

        let result = match prev_version {
            Some(ExpectedVersion::Nothing) => {
                sqlx::query(
                    //language=postgresql
                    r#"
                    INSERT INTO profile_transfer_request_events (version, id, event_name, data)
                    SELECT $1, $2, $3, $4
                    WHERE NOT EXISTS (SELECT 1 FROM profile_transfer_request_events WHERE id = $2)
                    "#,
                )
                .bind(version)
                .bind(command.id().as_ref())
                .bind(event_name)
                .bind(&data)
                .execute(&mut *con)
                .await
                .convert_error()?
            }
            Some(ExpectedVersion::At(prev)) => {
                sqlx::query(
                    //language=postgresql
                    r#"
                    INSERT INTO profile_transfer_request_events (version, id, event_name, data)
                    SELECT $1, $2, $3, $4
                    WHERE (SELECT MAX(version) FROM profile_transfer_request_events WHERE id = $2) = $5
                    "#,
                )
                .bind(version)
                .bind(command.id().as_ref())
                .bind(event_name)
                .bind(&data)
                .bind(prev.as_ref())
                .execute(&mut *con)
                .await
                .convert_error()?
            }
            None => {
                sqlx::query(
                    //language=postgresql
                    r#"
                    INSERT INTO profile_transfer_request_events (version, id, event_name, data)
                    VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(version)
                .bind(command.id().as_ref())
                .bind(event_name)
                .bind(&data)
                .execute(con)
                .await
                .convert_error()?
            }
        };

        if prev_version.is_some() && result.rows_affected() == 0 {
            return Err(
                Report::new(KernelError::Concurrency).attach_printable(format!(
                    "Concurrency conflict for event {}",
                    command.id().as_ref()
                )),
            );
        }

        Ok(())
    }
}

impl DependOnProfileTransferRequestEventStore for PostgresDatabase {
    type ProfileTransferRequestEventStore = PostgresProfileTransferRequestEventStore;

    fn profile_transfer_request_event_store(&self) -> &Self::ProfileTransferRequestEventStore {
        &PostgresProfileTransferRequestEventStore
    }
}

#[cfg(test)]
mod test {
    use crate::database::PostgresDatabase;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::event_store::{
        DependOnProfileTransferRequestEventStore, ProfileTransferRequestEventStore,
    };
    use kernel::prelude::entity::{
        AccountId, EventId, EventVersion, Nanoid, ProfileId, ProfileTransferRequest,
        ProfileTransferRequestId,
    };
    use kernel::KernelError;

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn requested_event_round_trips_by_id() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut connection = database.connection().await.unwrap();
        let request_id = ProfileTransferRequestId::new(kernel::generate_id());
        let command = ProfileTransferRequest::request(
            request_id.clone(),
            ProfileId::new(kernel::generate_id()),
            AccountId::new(kernel::generate_id()),
            AccountId::new(kernel::generate_id()),
            Nanoid::default(),
        );

        database
            .profile_transfer_request_event_store()
            .persist(&mut connection, &command)
            .await
            .unwrap();

        let events = database
            .profile_transfer_request_event_store()
            .find_by_id(&mut connection, &EventId::from(request_id), None)
            .await
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(&events[0].event, command.event());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn accept_with_wrong_expected_version_returns_concurrency_error() {
        kernel::ensure_generator_initialized();
        let database = PostgresDatabase::new().await.unwrap();
        let mut connection = database.connection().await.unwrap();
        let request_id = ProfileTransferRequestId::new(kernel::generate_id());
        let request = ProfileTransferRequest::request(
            request_id.clone(),
            ProfileId::new(kernel::generate_id()),
            AccountId::new(kernel::generate_id()),
            AccountId::new(kernel::generate_id()),
            Nanoid::default(),
        );
        database
            .profile_transfer_request_event_store()
            .persist_and_transform(&mut connection, request)
            .await
            .unwrap();
        let accept = ProfileTransferRequest::accept(request_id, EventVersion::new(0));

        let result = database
            .profile_transfer_request_event_store()
            .persist(&mut connection, &accept)
            .await;

        assert!(result.is_err_and(|error| error.current_context() == &KernelError::Concurrency));
    }
}
