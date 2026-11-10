use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::repository::{
    DependOnDomainControlRepository, DomainControlDeleteOutcome, DomainControlRepository,
};
use kernel::prelude::entity::{ControlDomain, DomainControl, DomainControlMode, ProfileId};
use kernel::KernelError;
use sqlx::types::time::OffsetDateTime;
use sqlx::PgConnection;

const COLUMNS: &str = "profile_id, domain, mode, revision, created_at, updated_at";

#[derive(sqlx::FromRow)]
struct DomainControlRow {
    profile_id: i64,
    domain: String,
    mode: String,
    revision: i64,
    created_at: OffsetDateTime,
    updated_at: OffsetDateTime,
}

impl TryFrom<DomainControlRow> for DomainControl {
    type Error = Report<KernelError>;

    fn try_from(row: DomainControlRow) -> Result<Self, Self::Error> {
        let domain = ControlDomain::parse(&row.domain).map_err(|e| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Stored domain control domain is invalid: {e}"))
        })?;
        let mode = DomainControlMode::from_value(&row.mode).ok_or_else(|| {
            Report::new(KernelError::Internal)
                .attach_printable(format!("Unknown domain control mode: {}", row.mode))
        })?;
        let revision = u64::try_from(row.revision).map_err(|_| {
            Report::new(KernelError::Internal).attach_printable("Negative domain control revision")
        })?;
        Ok(DomainControl::new(
            ProfileId::new(row.profile_id),
            domain,
            mode,
            revision,
            row.created_at,
            row.updated_at,
        ))
    }
}

/// A revision above `i64::MAX` can never be stored, so it can never match.
fn stored_revision(revision: u64) -> Option<i64> {
    i64::try_from(revision).ok()
}

pub struct PostgresDomainControlRepository;

impl DomainControlRepository for PostgresDomainControlRepository {
    type Connection = PostgresConnection;

    async fn find(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
    ) -> error_stack::Result<Option<DomainControl>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, DomainControlRow>(&format!(
            "SELECT {COLUMNS} FROM profile_domain_controls
             WHERE profile_id = $1 AND domain = $2 AND deleted_at IS NULL"
        ))
        .bind(profile_id.as_ref())
        .bind(domain.as_str())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
    }

    async fn find_by_profile(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
    ) -> error_stack::Result<Vec<DomainControl>, KernelError> {
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, DomainControlRow>(&format!(
            "SELECT {COLUMNS} FROM profile_domain_controls
             WHERE profile_id = $1 AND deleted_at IS NULL ORDER BY domain"
        ))
        .bind(profile_id.as_ref())
        .fetch_all(con)
        .await
        .convert_error()?
        .into_iter()
        .map(TryFrom::try_from)
        .collect()
    }

    async fn create(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
        mode: DomainControlMode,
    ) -> error_stack::Result<Option<DomainControl>, KernelError> {
        let con: &mut PgConnection = executor;
        // A live row makes the conflict-update WHERE false, so nothing is returned (= 409).
        // A tombstone is revived with revision + 1 so the revision stays monotonic.
        sqlx::query_as::<_, DomainControlRow>(&format!(
            "INSERT INTO profile_domain_controls (profile_id, domain, mode, revision)
             VALUES ($1, $2, $3, 1)
             ON CONFLICT (profile_id, domain) DO UPDATE
               SET mode = EXCLUDED.mode,
                   revision = profile_domain_controls.revision + 1,
                   created_at = now(),
                   updated_at = now(),
                   deleted_at = NULL
               WHERE profile_domain_controls.deleted_at IS NOT NULL
             RETURNING {COLUMNS}"
        ))
        .bind(profile_id.as_ref())
        .bind(domain.as_str())
        .bind(mode.as_str())
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
    }

    async fn replace(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
        mode: DomainControlMode,
        expected_revision: u64,
    ) -> error_stack::Result<Option<DomainControl>, KernelError> {
        let Some(expected_revision) = stored_revision(expected_revision) else {
            return Ok(None);
        };
        let con: &mut PgConnection = executor;
        sqlx::query_as::<_, DomainControlRow>(&format!(
            "UPDATE profile_domain_controls
             SET mode = $3, revision = revision + 1, updated_at = now()
             WHERE profile_id = $1 AND domain = $2 AND deleted_at IS NULL AND revision = $4
             RETURNING {COLUMNS}"
        ))
        .bind(profile_id.as_ref())
        .bind(domain.as_str())
        .bind(mode.as_str())
        .bind(expected_revision)
        .fetch_optional(con)
        .await
        .convert_error()?
        .map(TryFrom::try_from)
        .transpose()
    }

    async fn delete(
        &self,
        executor: &mut Self::Connection,
        profile_id: &ProfileId,
        domain: &ControlDomain,
        expected_revision: u64,
    ) -> error_stack::Result<DomainControlDeleteOutcome, KernelError> {
        let con: &mut PgConnection = executor;
        let deleted = match stored_revision(expected_revision) {
            Some(expected_revision) => sqlx::query(
                "UPDATE profile_domain_controls
                 SET deleted_at = now(), updated_at = now(), revision = revision + 1
                 WHERE profile_id = $1 AND domain = $2 AND deleted_at IS NULL AND revision = $3",
            )
            .bind(profile_id.as_ref())
            .bind(domain.as_str())
            .bind(expected_revision)
            .execute(&mut *con)
            .await
            .convert_error()?
            .rows_affected(),
            None => 0,
        };
        if deleted > 0 {
            return Ok(DomainControlDeleteOutcome::Deleted);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM profile_domain_controls
             WHERE profile_id = $1 AND domain = $2 AND deleted_at IS NULL)",
        )
        .bind(profile_id.as_ref())
        .bind(domain.as_str())
        .fetch_one(con)
        .await
        .convert_error()?;
        Ok(if exists {
            DomainControlDeleteOutcome::RevisionMismatch
        } else {
            DomainControlDeleteOutcome::NotFound
        })
    }
}

impl DependOnDomainControlRepository for PostgresDatabase {
    type DomainControlRepository = PostgresDomainControlRepository;

    fn domain_control_repository(&self) -> &Self::DomainControlRepository {
        &PostgresDomainControlRepository
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::interfaces::database::DatabaseConnection;
    use kernel::interfaces::read_model::{
        AccountReadModel, DependOnAccountReadModel, DependOnProfileReadModel, ProfileReadModel,
    };
    use kernel::prelude::entity::AccountId;
    use kernel::test_utils::{unique_account_name, AccountBuilder, ProfileBuilder};

    async fn seed_profile(db: &PostgresDatabase) -> ProfileId {
        let mut conn = db.connection().await.unwrap();
        let account = AccountBuilder::new()
            .id(AccountId::default())
            .name(unique_account_name())
            .build();
        let profile = ProfileBuilder::new()
            .id(ProfileId::new(kernel::generate_id()))
            .account_id(account.id().clone())
            .build();
        db.account_read_model()
            .create(&mut conn, &account)
            .await
            .unwrap();
        db.profile_read_model()
            .create(&mut conn, &profile)
            .await
            .unwrap();
        profile.id().clone()
    }

    fn domain(value: &str) -> ControlDomain {
        ControlDomain::parse(value).unwrap()
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn create_find_replace_delete_roundtrip() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let repo = db.domain_control_repository();
        let profile = seed_profile(&db).await;
        let other_profile = seed_profile(&db).await;
        let mut conn = db.connection().await.unwrap();
        let example = domain("example.com");

        assert!(repo
            .find(&mut conn, &profile, &example)
            .await
            .unwrap()
            .is_none());

        let created = repo
            .create(&mut conn, &profile, &example, DomainControlMode::Block)
            .await
            .unwrap()
            .expect("first create succeeds");
        assert_eq!(created.revision(), 1);
        assert_eq!(created.mode(), DomainControlMode::Block);
        assert_eq!(created.profile_id(), &profile);
        assert_eq!(created.domain(), &example);
        assert_eq!(
            repo.find(&mut conn, &profile, &example).await.unwrap(),
            Some(created.clone())
        );
        assert!(
            repo.create(&mut conn, &profile, &example, DomainControlMode::Mute)
                .await
                .unwrap()
                .is_none(),
            "creating over a live control conflicts"
        );

        // Other Profiles and other (even sub-) domains are untouched.
        assert!(repo
            .find(&mut conn, &other_profile, &example)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .find(&mut conn, &profile, &domain("sub.example.com"))
            .await
            .unwrap()
            .is_none());

        assert!(repo
            .replace(&mut conn, &profile, &example, DomainControlMode::Mute, 7)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .replace(
                &mut conn,
                &profile,
                &example,
                DomainControlMode::Mute,
                u64::MAX
            )
            .await
            .unwrap()
            .is_none());
        let replaced = repo
            .replace(&mut conn, &profile, &example, DomainControlMode::Mute, 1)
            .await
            .unwrap()
            .expect("matching revision replaces");
        assert_eq!(replaced.revision(), 2);
        assert_eq!(replaced.mode(), DomainControlMode::Mute);
        assert_eq!(replaced.created_at(), created.created_at());
        assert!(replaced.updated_at() >= created.updated_at());

        assert_eq!(
            repo.delete(&mut conn, &profile, &example, 1).await.unwrap(),
            DomainControlDeleteOutcome::RevisionMismatch
        );
        assert_eq!(
            repo.delete(&mut conn, &profile, &domain("missing.example"), 1)
                .await
                .unwrap(),
            DomainControlDeleteOutcome::NotFound
        );
        assert_eq!(
            repo.delete(&mut conn, &profile, &example, u64::MAX)
                .await
                .unwrap(),
            DomainControlDeleteOutcome::RevisionMismatch
        );
        assert_eq!(
            repo.delete(&mut conn, &profile, &example, 2).await.unwrap(),
            DomainControlDeleteOutcome::Deleted
        );
        assert!(repo
            .find(&mut conn, &profile, &example)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            repo.delete(&mut conn, &profile, &example, 3).await.unwrap(),
            DomainControlDeleteOutcome::NotFound
        );
        assert!(repo
            .replace(&mut conn, &profile, &example, DomainControlMode::Block, 2)
            .await
            .unwrap()
            .is_none());
        assert!(repo
            .replace(&mut conn, &profile, &example, DomainControlMode::Block, 3)
            .await
            .unwrap()
            .is_none());

        // Tombstone: the revision counter continues (1 create, 2 replace, 3 delete, 4 re-create).
        let recreated = repo
            .create(&mut conn, &profile, &example, DomainControlMode::Block)
            .await
            .unwrap()
            .expect("re-create after delete");
        assert_eq!(recreated.revision(), 4);
        assert!(recreated.created_at() >= replaced.created_at());
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn find_by_profile_lists_live_controls_ordered_by_domain() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let repo = db.domain_control_repository();
        let profile = seed_profile(&db).await;
        let other_profile = seed_profile(&db).await;
        let mut conn = db.connection().await.unwrap();
        for name in ["zeta.example", "alpha.example", "mid.example"] {
            repo.create(&mut conn, &profile, &domain(name), DomainControlMode::Mute)
                .await
                .unwrap()
                .unwrap();
        }
        repo.create(
            &mut conn,
            &other_profile,
            &domain("other.example"),
            DomainControlMode::Block,
        )
        .await
        .unwrap()
        .unwrap();
        repo.delete(&mut conn, &profile, &domain("mid.example"), 1)
            .await
            .unwrap();

        let listed = repo.find_by_profile(&mut conn, &profile).await.unwrap();
        assert_eq!(
            listed
                .iter()
                .map(|c| c.domain().as_str().to_string())
                .collect::<Vec<_>>(),
            vec!["alpha.example", "zeta.example"]
        );
        assert!(listed.iter().all(|c| c.profile_id() == &profile));
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn concurrent_writes_are_serialized_by_the_database() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let profile = seed_profile(&db).await;
        let target = domain("race.example");

        let mut creates = Vec::new();
        for _ in 0..10 {
            let db = db.clone();
            let profile = profile.clone();
            let target = target.clone();
            creates.push(tokio::spawn(async move {
                let mut conn = db.connection().await.unwrap();
                db.domain_control_repository()
                    .create(&mut conn, &profile, &target, DomainControlMode::Block)
                    .await
                    .unwrap()
                    .is_some()
            }));
        }
        let mut winners = 0;
        for handle in creates {
            winners += usize::from(handle.await.unwrap());
        }
        assert_eq!(winners, 1, "exactly one concurrent create wins");

        let mut replaces = Vec::new();
        for _ in 0..10 {
            let db = db.clone();
            let profile = profile.clone();
            let target = target.clone();
            replaces.push(tokio::spawn(async move {
                let mut conn = db.connection().await.unwrap();
                db.domain_control_repository()
                    .replace(&mut conn, &profile, &target, DomainControlMode::Mute, 1)
                    .await
                    .unwrap()
                    .is_some()
            }));
        }
        let mut winners = 0;
        for handle in replaces {
            winners += usize::from(handle.await.unwrap());
        }
        assert_eq!(winners, 1, "exactly one concurrent replace wins");
        let mut conn = db.connection().await.unwrap();
        let current = db
            .domain_control_repository()
            .find(&mut conn, &profile, &target)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.revision(), 2);
    }

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn rows_are_removed_with_their_profile_and_the_schema_rejects_unnormalized_domains() {
        kernel::ensure_generator_initialized();
        let db = PostgresDatabase::new().await.unwrap();
        let profile = seed_profile(&db).await;
        let mut conn = db.connection().await.unwrap();
        db.domain_control_repository()
            .create(
                &mut conn,
                &profile,
                &domain("cascade.example"),
                DomainControlMode::Block,
            )
            .await
            .unwrap()
            .unwrap();

        for bad in [
            "Upper.example",
            "trailing.example.",
            "",
            "has space.example",
            "*.example",
        ] {
            let con: &mut PgConnection = &mut conn;
            let result = sqlx::query(
                "INSERT INTO profile_domain_controls (profile_id, domain, mode, revision)
                 VALUES ($1, $2, 'mute', 1)",
            )
            .bind(profile.as_ref())
            .bind(bad)
            .execute(con)
            .await;
            assert!(result.is_err(), "{bad:?} must violate the domain check");
        }
        let con: &mut PgConnection = &mut conn;
        let result = sqlx::query(
            "INSERT INTO profile_domain_controls (profile_id, domain, mode, revision)
             VALUES ($1, 'x.example', 'ban', 1)",
        )
        .bind(profile.as_ref())
        .execute(con)
        .await;
        assert!(result.is_err(), "unknown mode must violate the mode check");

        let con: &mut PgConnection = &mut conn;
        sqlx::query("DELETE FROM profiles WHERE id = $1")
            .bind(profile.as_ref())
            .execute(&mut *con)
            .await
            .unwrap();
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM profile_domain_controls WHERE profile_id = $1",
        )
        .bind(profile.as_ref())
        .fetch_one(con)
        .await
        .unwrap();
        assert_eq!(remaining, 0, "controls cascade with their Profile");
    }
}
