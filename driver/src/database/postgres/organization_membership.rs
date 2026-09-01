use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnOrganizationMembershipRepository, OrganizationMembershipRepository,
};
use kernel::prelude::entity::{
    AccountId, CreatedAt, OrgRole, OrganizationMembership, OrganizationMembershipStatus,
};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;

#[derive(sqlx::FromRow)]
struct OrganizationMembershipRow {
    org_account_id: i64,
    member_account_id: i64,
    role: String,
    status: String,
    invited_by: i64,
    created_at: OffsetDateTime,
}

impl TryFrom<OrganizationMembershipRow> for OrganizationMembership {
    type Error = Report<KernelError>;

    fn try_from(value: OrganizationMembershipRow) -> Result<Self, Self::Error> {
        let role = match value.role.as_str() {
            "owner" => OrgRole::Owner,
            "admin" => OrgRole::Admin,
            "member" => OrgRole::Member,
            unknown => {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("Unknown organization role: {unknown}")))
            }
        };
        let status = match value.status.as_str() {
            "pending" => OrganizationMembershipStatus::Pending,
            "active" => OrganizationMembershipStatus::Active,
            unknown => {
                return Err(Report::new(KernelError::Internal).attach_printable(format!(
                    "Unknown organization membership status: {unknown}"
                )))
            }
        };
        Ok(OrganizationMembership::new(
            AccountId::new(value.org_account_id),
            AccountId::new(value.member_account_id),
            role,
            status,
            AccountId::new(value.invited_by),
            CreatedAt::new(value.created_at),
        ))
    }
}

fn role_value(role: OrgRole) -> &'static str {
    match role {
        OrgRole::Owner => "owner",
        OrgRole::Admin => "admin",
        OrgRole::Member => "member",
    }
}

fn status_value(status: OrganizationMembershipStatus) -> &'static str {
    match status {
        OrganizationMembershipStatus::Pending => "pending",
        OrganizationMembershipStatus::Active => "active",
    }
}

pub struct PostgresOrganizationMembershipRepository;

impl OrganizationMembershipRepository for PostgresOrganizationMembershipRepository {
    type Connection = PostgresConnection;

    async fn create(
        &self,
        executor: &mut Self::Connection,
        membership: &OrganizationMembership,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query(
            "INSERT INTO organization_members
             (org_account_id, member_account_id, role, status, invited_by, created_at)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(membership.org_account_id().as_ref())
        .bind(membership.member_account_id().as_ref())
        .bind(role_value(*membership.role()))
        .bind(status_value(*membership.status()))
        .bind(membership.invited_by().as_ref())
        .bind(membership.created_at().as_ref())
        .execute(con)
        .await
        .convert_error()?;
        Ok(())
    }

    async fn find(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
    ) -> error_stack::Result<Option<OrganizationMembership>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, OrganizationMembershipRow>(
            "SELECT org_account_id, member_account_id, role, status, invited_by, created_at
             FROM organization_members WHERE org_account_id = $1 AND member_account_id = $2",
        )
        .bind(org_account_id.as_ref())
        .bind(member_account_id.as_ref())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
    }

    async fn find_by_org(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, OrganizationMembershipRow>(
            "SELECT org_account_id, member_account_id, role, status, invited_by, created_at
             FROM organization_members WHERE org_account_id = $1 ORDER BY created_at",
        )
        .bind(org_account_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(TryFrom::try_from)
        .collect()
    }

    async fn find_by_member(
        &self,
        executor: &mut Self::Connection,
        member_account_id: &AccountId,
    ) -> error_stack::Result<Vec<OrganizationMembership>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, OrganizationMembershipRow>(
            "SELECT org_account_id, member_account_id, role, status, invited_by, created_at
             FROM organization_members WHERE member_account_id = $1 ORDER BY created_at",
        )
        .bind(member_account_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(TryFrom::try_from)
        .collect()
    }

    async fn update_role(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
        role: OrgRole,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            "UPDATE organization_members SET role = $3
             WHERE org_account_id = $1 AND member_account_id = $2",
        )
        .bind(org_account_id.as_ref())
        .bind(member_account_id.as_ref())
        .bind(role_value(role))
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound));
        }
        Ok(())
    }

    async fn update_status(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
        status: OrganizationMembershipStatus,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            "UPDATE organization_members SET status = $3
             WHERE org_account_id = $1 AND member_account_id = $2",
        )
        .bind(org_account_id.as_ref())
        .bind(member_account_id.as_ref())
        .bind(status_value(status))
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound));
        }
        Ok(())
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
        member_account_id: &AccountId,
    ) -> error_stack::Result<(), KernelError> {
        let con: &mut PgConnection = executor;
        let result = sqlx::query(
            "DELETE FROM organization_members WHERE org_account_id = $1 AND member_account_id = $2",
        )
        .bind(org_account_id.as_ref())
        .bind(member_account_id.as_ref())
        .execute(con)
        .await
        .convert_error()?;
        if result.rows_affected() == 0 {
            return Err(Report::new(KernelError::NotFound));
        }
        Ok(())
    }

    async fn count_active_owners(
        &self,
        executor: &mut Self::Connection,
        org_account_id: &AccountId,
    ) -> error_stack::Result<i64, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_scalar(
            "SELECT COUNT(*) FROM organization_members
             WHERE org_account_id = $1 AND role = 'owner' AND status = 'active'",
        )
        .bind(org_account_id.as_ref())
        .fetch_one(con)
        .await
        .convert_error()
    }
}

impl DependOnOrganizationMembershipRepository for PostgresDatabase {
    type OrganizationMembershipRepository = PostgresOrganizationMembershipRepository;

    fn organization_membership_repository(&self) -> &Self::OrganizationMembershipRepository {
        &PostgresOrganizationMembershipRepository
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
    use kernel::prelude::entity::{Account, AccountKind, EventVersion, Nanoid};
    use kernel::test_utils::AccountBuilder;

    async fn create_account(
        db: &PostgresDatabase,
        conn: &mut PostgresConnection,
        kind: AccountKind,
    ) -> Account {
        let account = AccountBuilder::new()
            .kind(kind)
            .name(kernel::test_utils::unique_account_name())
            .nanoid(Nanoid::default())
            .version(EventVersion::new(1))
            .build();
        db.account_read_model()
            .create(conn, &account)
            .await
            .unwrap();
        account
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn crud_roundtrip() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let mut conn = db.connection().await.unwrap();
        let org = create_account(&db, &mut conn, AccountKind::Organization).await;
        let member = create_account(&db, &mut conn, AccountKind::Personal).await;
        let inviter = create_account(&db, &mut conn, AccountKind::Personal).await;
        let membership = OrganizationMembership::new(
            org.id().clone(),
            member.id().clone(),
            OrgRole::Member,
            OrganizationMembershipStatus::Pending,
            inviter.id().clone(),
            CreatedAt::now(),
        );

        db.organization_membership_repository()
            .create(&mut conn, &membership)
            .await
            .unwrap();
        assert_eq!(
            db.organization_membership_repository()
                .find(&mut conn, org.id(), member.id())
                .await
                .unwrap(),
            Some(membership)
        );

        db.organization_membership_repository()
            .update_role(&mut conn, org.id(), member.id(), OrgRole::Admin)
            .await
            .unwrap();
        db.organization_membership_repository()
            .update_status(
                &mut conn,
                org.id(),
                member.id(),
                OrganizationMembershipStatus::Active,
            )
            .await
            .unwrap();
        let updated = db
            .organization_membership_repository()
            .find(&mut conn, org.id(), member.id())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.role(), &OrgRole::Admin);
        assert_eq!(updated.status(), &OrganizationMembershipStatus::Active);
        assert_eq!(
            db.organization_membership_repository()
                .find_by_org(&mut conn, org.id())
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            db.organization_membership_repository()
                .find_by_member(&mut conn, member.id())
                .await
                .unwrap()
                .len(),
            1
        );

        db.organization_membership_repository()
            .delete(&mut conn, org.id(), member.id())
            .await
            .unwrap();
        assert!(db
            .organization_membership_repository()
            .find(&mut conn, org.id(), member.id())
            .await
            .unwrap()
            .is_none());
    }
}
