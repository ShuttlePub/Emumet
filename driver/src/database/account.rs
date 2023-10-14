use crate::DriverError;
use kernel::prelude::entity::{AccountDomain, AccountName, CreatedAt, Id, IsBot, StellarAccount};
use kernel::KernelError;
use kernel::{interfaces::repository::AccountRepository, prelude::entity::Account};
use sqlx::{types::time::OffsetDateTime, PgConnection, Pool, Postgres};

#[derive(Debug, Clone)]
pub struct AccountDatabase {
    pool: Pool<Postgres>,
}

impl AccountDatabase {
    pub fn new(pool: Pool<Postgres>) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl AccountRepository for AccountDatabase {
    async fn find_by_id(&self, id: &Id<Account>) -> Result<Option<Account>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgAccountInternal::find_by_id(id, &mut con).await?;
        Ok(found)
    }

    async fn find_by_stellar_id(
        &self,
        stellar_id: &Id<StellarAccount>,
    ) -> Result<Vec<Account>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgAccountInternal::find_by_stellar_id(stellar_id, &mut con).await?;
        Ok(found)
    }

    async fn find_by_name(&self, name: &AccountName) -> Result<Option<Account>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgAccountInternal::find_by_name(name, &mut con).await?;
        Ok(found)
    }

    async fn find_by_domain(&self, domain: &AccountDomain) -> Result<Vec<Account>, KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        let found = PgAccountInternal::find_by_domain(domain, &mut con).await?;
        Ok(found)
    }

    async fn save(&self, account: &Account) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgAccountInternal::create(account, &mut con).await?;
        Ok(())
    }

    async fn update(&self, account: &Account) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgAccountInternal::update(account, &mut con).await?;
        Ok(())
    }

    async fn delete(&self, id: &Id<Account>) -> Result<(), KernelError> {
        let mut con = self.pool.acquire().await.map_err(DriverError::SqlX)?;
        PgAccountInternal::delete(id, &mut con).await?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
pub(in crate::database) struct AccountRow {
    id: i64,
    domain: String,
    name: String,
    is_bot: bool,
    created_at: OffsetDateTime,
}

fn to_account(row: AccountRow) -> Account {
    Account::new(
        Id::new(row.id),
        AccountDomain::new(row.domain),
        AccountName::new(row.name),
        IsBot::new(row.is_bot),
        CreatedAt::new(row.created_at),
    )
}

fn to_account_with_result(row: AccountRow) -> Result<Account, DriverError> {
    Ok(to_account(row))
}

pub(in crate::database) struct PgAccountInternal;

impl PgAccountInternal {
    pub async fn create(account: &Account, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(
            r#"INSERT INTO accounts (id, domain, name, is_bot, created_at) VALUES ($1, $2, $3, $4, $5)"#,
        ).bind(account.id().as_ref())
            .bind(account.domain().as_ref())
            .bind(account.name().as_ref())
            .bind(account.is_bot().as_ref())
            .bind(account.created_at().as_ref())
            .execute(con)
            .await?;

        Ok(())
    }

    pub async fn update(account: &Account, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(
            r#"UPDATE accounts SET domain = $1, name = $2, is_bot = $3, created_at = $4 WHERE id = $5"#,
        ).bind(account.domain().as_ref())
            .bind(account.name().as_ref())
            .bind(account.is_bot().as_ref())
            .bind(account.created_at().as_ref())
            .bind(account.id().as_ref())
            .execute(con)
            .await?;

        Ok(())
    }

    pub async fn delete(id: &Id<Account>, con: &mut PgConnection) -> Result<(), DriverError> {
        // language=sql
        sqlx::query(r#"DELETE FROM accounts WHERE id = $1"#)
            .bind(id.as_ref())
            .execute(con)
            .await?;

        Ok(())
    }

    pub async fn find_by_id(
        id: &Id<Account>,
        con: &mut PgConnection,
    ) -> Result<Option<Account>, DriverError> {
        // language=sql
        sqlx::query_as::<_, AccountRow>(
            r#"SELECT id, domain, name, is_bot, created_at FROM accounts WHERE id = $1"#,
        )
        .bind(id.as_ref())
        .fetch_optional(con)
        .await?
        .map(to_account_with_result)
        .transpose()
    }

    pub async fn find_by_stellar_id(
        stellar_id: &Id<StellarAccount>,
        con: &mut PgConnection,
    ) -> Result<Vec<Account>, DriverError> {
        // language=sql
        let accounts = sqlx::query_as::<_, AccountRow>(
            r#"SELECT id, domain, name, is_bot, created_at FROM accounts INNER JOIN stellar_emumet_accounts ON accounts.id = stellar_emumet_accounts.emumet_id WHERE stellar_emumet_accounts.emumet_id = $1"#
        )
            .bind(stellar_id.as_ref())
            .fetch_all(con)
            .await?
            .into_iter()
            .map(to_account)
            .collect::<Vec<Account>>();
        Ok(accounts)
    }

    pub async fn find_by_name(
        name: &AccountName,
        con: &mut PgConnection,
    ) -> Result<Option<Account>, DriverError> {
        // language=sql
        sqlx::query_as::<_, AccountRow>(
            r#"SELECT id, domain, name, is_bot, created_at FROM accounts WHERE name = $1"#,
        )
        .bind(name.as_ref())
        .fetch_optional(con)
        .await?
        .map(to_account_with_result)
        .transpose()
    }

    pub async fn find_by_domain(
        domain: &AccountDomain,
        con: &mut PgConnection,
    ) -> Result<Vec<Account>, DriverError> {
        // language=sql
        let accounts = sqlx::query_as::<_, AccountRow>(
            r#"SELECT id, domain, name, is_bot, created_at FROM accounts WHERE domain = $1"#,
        )
        .bind(domain.as_ref())
        .fetch_all(con)
        .await?
        .into_iter()
        .map(to_account)
        .collect::<Vec<Account>>();
        Ok(accounts)
    }
}
