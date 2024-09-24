use sqlx::PgConnection;
use uuid::Uuid;

use kernel::interfaces::modify::{DependOnProfileModifier, ProfileModifier};
use kernel::interfaces::query::{DependOnProfileQuery, ProfileQuery};
use kernel::prelude::entity::{
    AccountId, ImageId, Profile, ProfileDisplayName, ProfileId, ProfileSummary,
};
use kernel::KernelError;

use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;

#[derive(sqlx::FromRow)]
struct ProfileRow {
    id: Uuid,
    account_id: Uuid,
    display: Option<String>,
    summary: Option<String>,
    icon_id: Option<Uuid>,
    banner_id: Option<Uuid>,
}

impl From<ProfileRow> for Profile {
    fn from(value: ProfileRow) -> Self {
        Profile::new(
            ProfileId::new(value.id),
            AccountId::new(value.id),
            value.display.map(ProfileDisplayName::new),
            value.summary.map(ProfileSummary::new),
            value.icon_id.map(ImageId::new),
            value.banner_id.map(ImageId::new),
        )
    }
}

pub struct PostgresProfileRepository;

impl ProfileQuery for PostgresProfileRepository {
    type Transaction = PostgresConnection;

    async fn find_by_id(
        &self,
        transaction: &mut Self::Transaction,
        id: &ProfileId,
    ) -> error_stack::Result<Option<Profile>, KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query_as::<_, ProfileRow>(
            //language=postgresql
            r#"
            SELECT id, account_id, display, summary, icon_id, banner_id FROM profiles WHERE id = $1
            "#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()
        .map(|option| option.map(|row| row.into()))
    }
}

impl DependOnProfileQuery for PostgresDatabase {
    type ProfileQuery = PostgresProfileRepository;

    fn profile_query(&self) -> &Self::ProfileQuery {
        &PostgresProfileRepository
    }
}

impl ProfileModifier for PostgresProfileRepository {
    type Transaction = PostgresConnection;

    async fn create(
        &self,
        transaction: &mut Self::Transaction,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            INSERT INTO profiles (id, account_id, display, summary, icon_id, banner_id)
            VALUES ($1, $2, $3, $4, $5, $6)
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
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn update(
        &self,
        transaction: &mut Self::Transaction,
        profile: &Profile,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = transaction;
        sqlx::query(
            //language=postgresql
            r#"
            UPDATE profiles SET display = $2, summary = $3, icon_id = $4, banner_id = $5 WHERE account_id = $1
            "#
        )
            .bind(profile.id().as_ref())
            .bind(profile.display_name().as_ref().map(ProfileDisplayName::as_ref))
            .bind(profile.summary().as_ref().map(ProfileSummary::as_ref))
            .bind(profile.icon().as_ref().map(ImageId::as_ref))
            .bind(profile.banner().as_ref().map(ImageId::as_ref))
            .execute(con)
            .await
            .convert_error()?;
        Ok(())
    }
}

impl DependOnProfileModifier for PostgresDatabase {
    type ProfileModifier = PostgresProfileRepository;

    fn profile_modifier(&self) -> &Self::ProfileModifier {
        &PostgresProfileRepository
    }
}

#[cfg(test)]
mod test {
    mod query {
        use time::OffsetDateTime;
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{
            AccountModifier, DependOnAccountModifier, DependOnProfileModifier, ProfileModifier,
        };
        use kernel::interfaces::query::{DependOnProfileQuery, ProfileQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            CreatedAt, Profile, ProfileDisplayName, ProfileId, ProfileSummary,
        };

        use crate::database::PostgresDatabase;

        #[tokio::test]
        async fn find_by_id() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(Uuid::now_v7());
            let account_id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                account_id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            let profile = Profile::new(
                profile_id.clone(),
                account_id,
                Some(ProfileDisplayName::new("display name")),
                Some(ProfileSummary::new("summary")),
                None,
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_modifier()
                .create(&mut transaction, &profile)
                .await
                .unwrap();

            let result = database
                .profile_query()
                .find_by_id(&mut transaction, &profile_id)
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Profile::id), Some(profile.id()));
            database
                .account_modifier()
                .delete(&mut transaction, account.id())
                .await
                .unwrap();
        }
    }

    mod modify {
        use time::OffsetDateTime;
        use uuid::Uuid;

        use kernel::interfaces::database::DatabaseConnection;
        use kernel::interfaces::modify::{
            AccountModifier, DependOnAccountModifier, DependOnProfileModifier, ProfileModifier,
        };
        use kernel::interfaces::query::{DependOnProfileQuery, ProfileQuery};
        use kernel::prelude::entity::{
            Account, AccountId, AccountIsBot, AccountName, AccountPrivateKey, AccountPublicKey,
            CreatedAt, Profile, ProfileDisplayName, ProfileId, ProfileSummary,
        };

        use crate::database::PostgresDatabase;

        #[tokio::test]
        async fn create() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(Uuid::now_v7());
            let account_id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                account_id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            let profile = Profile::new(
                profile_id,
                account_id,
                Some(ProfileDisplayName::new("display name")),
                Some(ProfileSummary::new("summary")),
                None,
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_modifier()
                .create(&mut transaction, &profile)
                .await
                .unwrap();
            database
                .account_modifier()
                .delete(&mut transaction, account.id())
                .await
                .unwrap();
        }

        #[tokio::test]
        async fn update() {
            let database = PostgresDatabase::new().await.unwrap();
            let mut transaction = database.begin_transaction().await.unwrap();

            let profile_id = ProfileId::new(Uuid::now_v7());
            let account_id = AccountId::new(Uuid::now_v7());
            let account = Account::new(
                account_id.clone(),
                AccountName::new("test"),
                AccountPrivateKey::new("test"),
                AccountPublicKey::new("test"),
                AccountIsBot::new(false),
                CreatedAt::new(OffsetDateTime::now_utc()),
                None,
            );
            let profile = Profile::new(
                profile_id.clone(),
                account_id.clone(),
                Some(ProfileDisplayName::new("display name")),
                Some(ProfileSummary::new("summary")),
                None,
                None,
            );
            database
                .account_modifier()
                .create(&mut transaction, &account)
                .await
                .unwrap();
            database
                .profile_modifier()
                .create(&mut transaction, &profile)
                .await
                .unwrap();

            let updated_profile = Profile::new(
                profile_id.clone(),
                account_id,
                Some(ProfileDisplayName::new("updated display name")),
                Some(ProfileSummary::new("updated summary")),
                None,
                None,
            );
            database
                .profile_modifier()
                .update(&mut transaction, &updated_profile)
                .await
                .unwrap();

            let result = database
                .profile_query()
                .find_by_id(&mut transaction, &profile_id)
                .await
                .unwrap();
            assert_eq!(result.as_ref().map(Profile::id), Some(updated_profile.id()));
            database
                .account_modifier()
                .delete(&mut transaction, account.id())
                .await
                .unwrap();
        }
    }
}
