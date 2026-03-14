use sqlx::PgConnection;

use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
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

impl From<ProfileRow> for Profile {
    fn from(value: ProfileRow) -> Self {
        Profile::new(
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
    type Executor = PostgresConnection;

    async fn find_by_id(
        &self,
        executor: &mut Self::Executor,
        id: &ProfileId,
    ) -> error_stack::Result<Option<Profile>, KernelError> {
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
        .map(|option| option.map(Profile::from))
    }

    async fn find_by_account_id(
        &self,
        executor: &mut Self::Executor,
        account_id: &AccountId,
    ) -> error_stack::Result<Option<Profile>, KernelError> {
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
        .map(|option| option.map(Profile::from))
    }

    async fn find_by_account_ids(
        &self,
        executor: &mut Self::Executor,
        account_ids: &[AccountId],
    ) -> error_stack::Result<Vec<Profile>, KernelError> {
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
        .map(|rows| rows.into_iter().map(Profile::from).collect())
    }

    async fn create(
        &self,
        executor: &mut Self::Executor,
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
        executor: &mut Self::Executor,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
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
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Executor,
        profile_id: &ProfileId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            //language=postgresql
            r#"
            DELETE FROM profiles WHERE id = $1
            "#,
        )
        .bind(profile_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
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
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            CreatedAt, EventVersion, Nanoid, Profile, ProfileDisplayName, ProfileId,
            ProfileSummary,
        };

        use crate::database::PostgresDatabase;

        fn make_account(account_id: AccountId) -> Account {
            Account::new(
                account_id,
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                Default::default(),
                None,
                EventVersion::default(),
                Nanoid::default(),
                CreatedAt::now(),
            )
        }

        fn make_profile(profile_id: ProfileId, account_id: AccountId) -> Profile {
            Profile::new(
                profile_id,
                account_id,
                Some(ProfileDisplayName::new("display name")),
                Some(ProfileSummary::new("summary")),
                None,
                None,
                EventVersion::default(),
                Nanoid::default(),
            )
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = make_account(account_id.clone());
            let profile = make_profile(profile_id.clone(), account_id.clone());

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut transaction, &profile)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_id(&mut transaction, &profile_id)
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Profile::id), Some(profile.id()));

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn find_by_account_id() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = make_account(account_id.clone());
            let profile = make_profile(profile_id.clone(), account_id.clone());

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut transaction, &profile)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_account_id(&mut transaction, &account_id)
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Profile::id), Some(profile.id()));

            // Non-existent account_id returns None
            let not_found = database
                .profile_read_model()
                .find_by_account_id(&mut transaction, &AccountId::default())
                .await
                .unwrap();
            assert!(not_found.is_none());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn create() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = make_account(account_id.clone());
            let profile = make_profile(profile_id.clone(), account_id.clone());

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut transaction, &profile)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_id(&mut transaction, &profile_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.id(), profile.id());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn update() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = make_account(account_id.clone());
            let profile = make_profile(profile_id.clone(), account_id.clone());

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut transaction, &profile)
                .await
                .unwrap();

            let updated_profile = Profile::new(
                profile_id.clone(),
                account_id.clone(),
                Some(ProfileDisplayName::new("updated display name")),
                Some(ProfileSummary::new("updated summary")),
                None,
                None,
                EventVersion::default(),
                Nanoid::default(),
            );
            database
                .profile_read_model()
                .update(&mut transaction, &updated_profile)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_id(&mut transaction, &profile_id)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.id(), updated_profile.id());
            assert_eq!(result.display_name(), updated_profile.display_name());
            assert_eq!(result.summary(), updated_profile.summary());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[test_with::env(DATABASE_URL)]
        #[tokio::test]
        async fn delete() {
            kernel::ensure_generator_initialized();
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(kernel::generate_id());
            let account_id = AccountId::default();
            let account = make_account(account_id.clone());
            let profile = make_profile(profile_id.clone(), account_id.clone());

            database
                .account_read_model()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_read_model()
                .create(&mut transaction, &profile)
                .await
                .unwrap();

            database
                .profile_read_model()
                .delete(&mut transaction, &profile_id)
                .await
                .unwrap();

            let result = database
                .profile_read_model()
                .find_by_id(&mut transaction, &profile_id)
                .await
                .unwrap();
            assert!(result.is_none());

            database
                .account_read_model()
                .deactivate(&mut transaction, account.id())
                .await
                .unwrap();
        }
    }
}
