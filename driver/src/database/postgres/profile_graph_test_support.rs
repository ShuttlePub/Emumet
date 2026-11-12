//! Profile social graph の実 DB テストが使う seed 関数。独立 DB 自体は `isolated_db` にある。

pub(crate) use super::isolated_db::{IsolatedDatabase, RESET_SQL};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::read_model::{
    AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
};
use kernel::prelude::entity::{AccountId, AccountKind, ProfileId};
use kernel::test_utils::{AccountBuilder, ProfileBuilder};

use super::PostgresDatabase;

/// 個人 Account とその Profile を作る。
pub(crate) async fn seed_personal_profile(database: &PostgresDatabase) -> (AccountId, ProfileId) {
    let account_id = AccountId::default();
    let account = AccountBuilder::new().id(account_id.clone()).build();
    seed_profile_on(database, account, &account_id).await
}

/// 組織 Account (同一 Account に複数 Profile を持てる) を作る。
pub(crate) async fn seed_organization_account(database: &PostgresDatabase) -> AccountId {
    let account_id = AccountId::default();
    let account = AccountBuilder::new()
        .id(account_id.clone())
        .kind(AccountKind::Organization)
        .build();
    let mut conn = database.connection().await.unwrap();
    database
        .account_read_model()
        .create(&mut conn, &account)
        .await
        .unwrap();
    account_id
}

/// 既存 Account に Profile を 1 つ追加する (組織 Account では複数可)。
pub(crate) async fn seed_profile_for(
    database: &PostgresDatabase,
    account_id: &AccountId,
) -> ProfileId {
    let profile_id = ProfileId::new(kernel::generate_id());
    let profile = ProfileBuilder::new()
        .id(profile_id.clone())
        .account_id(account_id.clone())
        .build();
    let mut conn = database.connection().await.unwrap();
    database
        .profile_read_model()
        .create(&mut conn, &profile)
        .await
        .unwrap();
    profile_id
}

async fn seed_profile_on(
    database: &PostgresDatabase,
    account: kernel::prelude::entity::Account,
    account_id: &AccountId,
) -> (AccountId, ProfileId) {
    let mut conn = database.connection().await.unwrap();
    database
        .account_read_model()
        .create(&mut conn, &account)
        .await
        .unwrap();
    drop(conn);
    let profile_id = seed_profile_for(database, account_id).await;
    (account_id.clone(), profile_id)
}
