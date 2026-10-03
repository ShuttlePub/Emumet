use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::read_model::{
    DependOnProfileTransferRequestReadModel, ProfileTransferRequestProjection,
    ProfileTransferRequestReadModel,
};
use kernel::prelude::entity::{
    AccountId, EventVersion, Nanoid, ProfileId, ProfileTransferRequest, ProfileTransferRequestId,
    ProfileTransferStatus,
};
use kernel::KernelError;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct ProfileTransferRequestRow {
    id: i64,
    profile_id: i64,
    from_account_id: i64,
    to_org_account_id: i64,
    status: String,
    version: i64,
    nanoid: String,
}

impl TryFrom<ProfileTransferRequestRow> for ProfileTransferRequestProjection {
    type Error = Report<KernelError>;

    fn try_from(value: ProfileTransferRequestRow) -> Result<Self, Self::Error> {
        let status = match value.status.as_str() {
            "pending" => ProfileTransferStatus::Pending,
            "accepted" => ProfileTransferStatus::Accepted,
            "rejected" => ProfileTransferStatus::Rejected,
            "cancelled" => ProfileTransferStatus::Cancelled,
            status => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Unknown profile transfer request status: {status}"
                )))
            }
        };
        Ok(ProfileTransferRequestProjection::new(
            ProfileTransferRequestId::new(value.id),
            ProfileId::new(value.profile_id),
            AccountId::new(value.from_account_id),
            AccountId::new(value.to_org_account_id),
            status,
            EventVersion::new(value.version),
            Nanoid::new(value.nanoid),
        ))
    }
}

fn status_value(status: &ProfileTransferStatus) -> &'static str {
    match status {
        ProfileTransferStatus::Pending => "pending",
        ProfileTransferStatus::Accepted => "accepted",
        ProfileTransferStatus::Rejected => "rejected",
        ProfileTransferStatus::Cancelled => "cancelled",
    }
}

pub struct PostgresProfileTransferRequestReadModel;

impl ProfileTransferRequestReadModel for PostgresProfileTransferRequestReadModel {
    type Connection = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileTransferRequestId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileTransferRequestRow>(
            "SELECT id, profile_id, from_account_id, to_org_account_id, status, version, nanoid
             FROM profile_transfer_requests WHERE id = $1",
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileTransferRequestId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        self.find_by_id(executor, id).await
    }

    async fn find_by_nanoid(
        &self,
        executor: &mut Self::Connection,
        nanoid: &Nanoid<ProfileTransferRequest>,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileTransferRequestRow>(
            "SELECT id, profile_id, from_account_id, to_org_account_id, status, version, nanoid
             FROM profile_transfer_requests WHERE nanoid = $1",
        )
        .bind(nanoid.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
    }

    async fn find_pending_by_profile_id(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileTransferRequestProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileTransferRequestRow>(
            "SELECT id, profile_id, from_account_id, to_org_account_id, status, version, nanoid
             FROM profile_transfer_requests WHERE profile_id = $1 AND status = 'pending' LIMIT 1",
        )
        .bind(profile_id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
        request: &ProfileTransferRequest,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            "INSERT INTO profile_transfer_requests
             (id, profile_id, from_account_id, to_org_account_id, status, version, nanoid)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(request.id().as_ref())
        .bind(request.profile_id().as_ref())
        .bind(request.from_account_id().as_ref())
        .bind(request.to_org_account_id().as_ref())
        .bind(status_value(request.status()))
        .bind(request.version().as_ref())
        .bind(request.nanoid().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Connection,
        request: &ProfileTransferRequest,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            "UPDATE profile_transfer_requests
             SET status = $2, version = $3
             WHERE id = $1",
        )
        .bind(request.id().as_ref())
        .bind(status_value(request.status()))
        .bind(request.version().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound).attach_printable(format!(
                "Target profile transfer request not found for update: {}",
                request.id().as_ref()
            )));
        }
        Ok(())
    }
}

impl DependOnProfileTransferRequestReadModel for PostgresDatabase {
    type ProfileTransferRequestReadModel = PostgresProfileTransferRequestReadModel;

    fn profile_transfer_request_read_model(&self) -> &Self::ProfileTransferRequestReadModel {
        &PostgresProfileTransferRequestReadModel
    }
}

#[cfg(test)]
mod test {
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
    use kernel::interfaces::read_model::{
        DependOnProfileReadModel, DependOnProfileTransferRequestReadModel, ProfileReadModel,
        ProfileTransferRequestReadModel,
    };
    use kernel::prelude::entity::{
        AccountKind, EventVersion, Nanoid, ProfileId, ProfileTransferRequest,
        ProfileTransferRequestId,
    };
    use kernel::test_utils::{AccountBuilder, ProfileBuilder};

    use crate::database::PostgresDatabase;

    async fn create_account(
        db: &PostgresDatabase,
        kind: AccountKind,
    ) -> kernel::prelude::entity::Account {
        let mut conn = db.connection().await.unwrap();
        let account = AccountBuilder::new()
            .kind(kind)
            .name(kernel::test_utils::unique_account_name())
            .nanoid(Nanoid::default())
            .version(EventVersion::new(1))
            .build();
        db.account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();
        account
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn create_find_update_roundtrip() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let from = create_account(&db, AccountKind::Personal).await;
        let to = create_account(&db, AccountKind::Organization).await;
        let owner_profile = ProfileBuilder::new().account_id(from.id().clone()).build();
        db.profile_read_model()
            .create(&mut conn, &owner_profile)
            .await
            .unwrap();
        let request_id = ProfileTransferRequestId::new(kernel::generate_id());
        let command = ProfileTransferRequest::request(
            request_id.clone(),
            owner_profile.id().clone(),
            from.id().clone(),
            to.id().clone(),
            Nanoid::default(),
        );
        let mut request = None;
        kernel::interfaces::event::EventApplier::apply(
            &mut request,
            kernel::prelude::entity::EventEnvelope::new(
                kernel::prelude::entity::EventId::from(request_id.clone()),
                command.event().clone(),
                EventVersion::new(1),
            ),
        )
        .unwrap();
        let request = request.unwrap();

        db.profile_transfer_request_read_model()
            .create(&mut conn, &request)
            .await
            .unwrap();

        let found = db
            .profile_transfer_request_read_model()
            .find_by_id(&mut conn, &request_id)
            .await
            .unwrap();
        assert!(found.is_some());
        let found = found.unwrap();
        assert_eq!(found.profile_id(), request.profile_id());
        assert_eq!(found.from_account_id(), request.from_account_id());
        assert_eq!(found.to_org_account_id(), request.to_org_account_id());
        assert_eq!(
            found.status(),
            &kernel::prelude::entity::ProfileTransferStatus::Pending
        );

        let by_nanoid = db
            .profile_transfer_request_read_model()
            .find_by_nanoid(&mut conn, found.nanoid())
            .await
            .unwrap();
        assert_eq!(by_nanoid.as_ref().map(|r| r.id()), Some(request.id()));

        let pending = db
            .profile_transfer_request_read_model()
            .find_pending_by_profile_id(&mut conn, owner_profile.id())
            .await
            .unwrap();
        assert_eq!(pending.as_ref().map(|r| r.id()), Some(request.id()));

        let accept = ProfileTransferRequest::accept(
            request_id.clone(),
            EventVersion::new(request.version().as_ref().to_owned()),
        );
        let mut accepted = Some(request);
        kernel::interfaces::event::EventApplier::apply(
            &mut accepted,
            kernel::prelude::entity::EventEnvelope::new(
                kernel::prelude::entity::EventId::from(request_id.clone()),
                accept.event().clone(),
                EventVersion::new(2),
            ),
        )
        .unwrap();
        let accepted = accepted.unwrap();
        db.profile_transfer_request_read_model()
            .update(&mut conn, &accepted)
            .await
            .unwrap();
        let updated = db
            .profile_transfer_request_read_model()
            .find_by_id(&mut conn, &request_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            updated.status(),
            &kernel::prelude::entity::ProfileTransferStatus::Accepted
        );

        db.account_read_model()
            .deactivate(&mut conn, from.id())
            .await
            .unwrap();
        db.account_read_model()
            .deactivate(&mut conn, to.id())
            .await
            .unwrap();
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn partial_unique_personal_profile_blocks_second_personal_profile() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let personal = create_account(&db, AccountKind::Personal).await;
        let first = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(personal.id().clone())
            .build();
        db.profile_read_model()
            .create(&mut conn, &first)
            .await
            .unwrap();

        let second = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(personal.id().clone())
            .build();
        let result = db.profile_read_model().create(&mut conn, &second).await;
        assert!(result.is_err());
        let error_string = format!("{result:?}");
        assert!(
            error_string.contains("unique") || error_string.contains("Unique"),
            "expected unique violation, got: {error_string}"
        );

        db.account_read_model()
            .deactivate(&mut conn, personal.id())
            .await
            .unwrap();
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn organization_may_have_multiple_profiles() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let org = create_account(&db, AccountKind::Organization).await;
        let first = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(org.id().clone())
            .build();
        let second = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(org.id().clone())
            .build();
        db.profile_read_model()
            .create(&mut conn, &first)
            .await
            .unwrap();
        db.profile_read_model()
            .create(&mut conn, &second)
            .await
            .unwrap();

        let profiles = db
            .profile_read_model()
            .find_by_account_id(&mut conn, org.id())
            .await
            .unwrap()
            .map(|_| 1)
            .unwrap_or(0);
        assert_eq!(profiles, 1);

        db.account_read_model()
            .deactivate(&mut conn, org.id())
            .await
            .unwrap();
    }
}
