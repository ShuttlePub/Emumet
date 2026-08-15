use sqlx::PgConnection;

use error_stack::Report;
use kernel::interfaces::read_model::{
    DependOnProfileReadModel, ProfileProjection, ProfileReadModel,
};
use kernel::prelude::entity::{
    AccountId, EventVersion, ImageId, Nanoid, Profile, ProfileDisplayName, ProfileId,
    ProfileSummary,
};
use kernel::KernelError;

use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;

#[derive(sqlx::FromRow)]
struct ProfileRow {
    id: i64,
    account_id: i64,
    display: Option<String>,
    summary: Option<String>,
    icon_id: Option<i64>,
    banner_id: Option<i64>,
    version: i64,
    nanoid: String,
}

impl From<ProfileRow> for ProfileProjection {
    fn from(value: ProfileRow) -> Self {
        ProfileProjection::new(
            ProfileId::new(value.id),
            AccountId::new(value.account_id),
            value.display.map(ProfileDisplayName::new),
            value.summary.map(ProfileSummary::new),
            value.icon_id.map(ImageId::new),
            value.banner_id.map(ImageId::new),
            EventVersion::new(value.version),
            Nanoid::new(value.nanoid),
        )
    }
}

pub struct PostgresProfileReadModel;

impl ProfileReadModel for PostgresProfileReadModel {
    type Connection = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileRow>(
            //language=postgresql
            r#"
            SELECT id, account_id, display, summary, icon_id, banner_id, version, nanoid
            FROM profiles WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(ProfileProjection::from))
    }

    async fn find_by_id_unfiltered(
        &self,
        executor: &mut Self::Connection,
        id: &ProfileId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        self.find_by_id(executor, id).await
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Connection,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<ProfileProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, ProfileRow>(
            //language=postgresql
            r#"
            SELECT id, account_id, display, summary, icon_id, banner_id, version, nanoid
            FROM profiles WHERE account_id = $1
            "#,
        )
        .bind(account_id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(ProfileProjection::from))
    }

    async fn find_by_account_ids(
        &self,
        executor: &mut Self::Connection,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<ProfileProjection>, KernelError> {
        let con: &mut PgConnection = executor;
        let ids: Vec<i64> = account_ids.iter().map(|id| *id.as_ref()).collect();
        sqlx::query_as::<_, ProfileRow>(
            //language=postgresql
            r#"
            SELECT id, account_id, display, summary, icon_id, banner_id, version, nanoid
            FROM profiles WHERE account_id = ANY($1)
            "#,
        )
        .bind(&ids)
        .fetch_all(con)
        .await
        .convert_error()
        .map(|rows| rows.into_iter().map(ProfileProjection::from).collect())
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profiles (id, account_id, display, summary, icon_id, banner_id, version, nanoid)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(profile.id().as_ref())
        .bind(profile.account_id().as_ref())
        .bind(
            profile
                .display_name()
                .as_ref()
                .map(ProfileDisplayName::as_ref),
        )
        .bind(profile.summary().as_ref().map(ProfileSummary::as_ref))
        .bind(profile.icon().as_ref().map(ImageId::as_ref))
        .bind(profile.banner().as_ref().map(ImageId::as_ref))
        .bind(profile.version().as_ref())
        .bind(profile.nanoid().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        executor: &mut Self::Connection,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            UPDATE profiles SET display = $2, summary = $3, icon_id = $4, banner_id = $5, version = $6
            WHERE id = $1
            "#,
        )
        .bind(profile.id().as_ref())
        .bind(
            profile
                .display_name()
                .as_ref()
                .map(ProfileDisplayName::as_ref),
        )
        .bind(profile.summary().as_ref().map(ProfileSummary::as_ref))
        .bind(profile.icon().as_ref().map(ImageId::as_ref))
        .bind(profile.banner().as_ref().map(ImageId::as_ref))
        .bind(profile.version().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target profile not found for update"));
        }
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM profiles WHERE id = $1
            "#,
        )
        .bind(profile_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound)
                .attach_printable("Target profile not found for delete"));
        }
        Ok(())
    }
}

impl DependOnProfileReadModel for PostgresDatabase {
    type ProfileReadModel = PostgresProfileReadModel;

    fn profile_read_model(&self) -> &Self::ProfileReadModel {
        &PostgresProfileReadModel
    }
}

#[cfg(test)]
mod test {
    mod read_model {
        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::read_model::{
            AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
        };
        use kernel::prelude::entity::{AccountId, EventVersion, ProfileId};
        use kernel::test_utils::{AccountBuilder, ProfileBuilder};

        use crate::database::PostgresDatabase;

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let profile = ProfileBuilder::new()
                .id(profile_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut conn, &profile)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_id(&mut conn, &profile_id)
                .await
                .unwrap();
            assert!(result.is_some());
            let result = result.unwrap();
            assert_eq!(result.id(), &profile_id);
            assert_eq!(result.display_name(), profile.display_name());
            assert_eq!(result.summary(), profile.summary());

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_account_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let profile = ProfileBuilder::new()
                .id(profile_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut conn, &profile)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_account_id(&mut conn, &account_id)
                .await
                .unwrap();
            assert!(result.is_some());
            let result = result.unwrap();
            assert_eq!(result.id(), &profile_id);

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update_and_find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let profile = ProfileBuilder::new()
                .id(profile_id.clone())
                .account_id(account_id.clone())
                .display_name(Some("old".to_string()))
                .summary(Some("old summary".to_string()))
                .build();
            let updated_profile = ProfileBuilder::new()
                .id(profile_id.clone())
                .account_id(account_id.clone())
                .display_name(Some("new".to_string()))
                .summary(Some("new summary".to_string()))
                .version(EventVersion::new(2))
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut conn, &profile)
                .await
                .unwrap();
            database
                .profile_read_model()
                .update(&mut conn, &updated_profile)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_id(&mut conn, &profile_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.id(), updated_profile.id());
            assert_eq!(result.display_name(), updated_profile.display_name());
            assert_eq!(result.summary(), updated_profile.summary());

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut conn = database.connection().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = AccountBuilder::new().id(account_id.clone()).build();
            let profile = ProfileBuilder::new()
                .id(profile_id.clone())
                .account_id(account_id.clone())
                .build();

            database
                .account_read_model()
                .create(&mut conn, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut conn, &profile)
                .await
                .unwrap();

            database
                .profile_read_model()
                .delete(&mut conn, &profile_id)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_id(&mut conn, &profile_id)
                .await
                .unwrap();
            assert!(result.is_none());

            database
                .account_read_model()
                .deactivate(&mut conn, account.id())
                .await
                .unwrap();
        }
    }
}
