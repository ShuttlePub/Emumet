//! Offline, pre-P4 recovery. Connecting through `from_pool` never runs migrations.
//! No Account reactivation, projection restoration, key generation, or federation writes.

use crate::database::{PostgresConnection, PostgresDatabase};
use crate::ConvertError;
use error_stack::Report;
use kernel::interfaces::database::{Transaction, TransactionalDatabaseConnection};
use kernel::interfaces::event::EventApplier;
use kernel::interfaces::event_store::{
    AccountEventStore, DependOnAccountEventStore, DependOnProfileEventStore, ProfileEventStore,
};
use kernel::prelude::entity::{Account, EventId, Profile, ProfileId, ProfileUsername};
use kernel::KernelError;
use serde::{Deserialize, Serialize};
use sqlx::{PgConnection, PgPool};

const FIRST_P4: i64 = 20261120000000;
const AUDIT_DDL: &str = "CREATE TABLE IF NOT EXISTS profile_cutover_recovery_audit (
    request_id TEXT PRIMARY KEY,
    request JSONB NOT NULL,
    result JSONB NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp()
)";

fn refuse(message: impl Into<String>) -> Report<KernelError> {
    Report::new(KernelError::Rejected).attach_printable(message.into())
}

/// IDs and versions are strings in JSON: Snowflakes must not lose precision in JSON tooling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileDiagnostic {
    pub profile_id: String,
    pub nanoid: Option<String>,
    pub username: Option<String>,
    pub profile_version: Option<String>,
    pub account_id: String,
    pub account_nanoid: Option<String>,
    pub account_version: Option<String>,
    pub account_deactivated: Option<bool>,
    pub account_status: Option<String>,
    pub profile_projection_exists: bool,
    pub account_projection_exists: bool,
    pub problem: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Diagnosis {
    pub profiles: Vec<ProfileDiagnostic>,
    pub missing_usernames: usize,
    pub problems: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsernameRecoveryRequest {
    pub request_id: String,
    pub operator: String,
    pub reason: String,
    pub profile_id: i64,
    pub expected_version: i64,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoveryResult {
    pub status: String,
    pub profile: ProfileDiagnostic,
    pub new_version: Option<String>,
}

async fn check_schema(con: &mut PgConnection) -> error_stack::Result<(), KernelError> {
    let baseline: bool = sqlx::query_scalar(
        "SELECT to_regclass('public._sqlx_migrations') IS NOT NULL
        AND to_regclass('public.profiles') IS NOT NULL
        AND to_regclass('public.profile_events') IS NOT NULL
        AND to_regclass('public.account_events') IS NOT NULL
        AND to_regclass('public.accounts') IS NOT NULL
        AND to_regclass('public.profile_events_username_unique') IS NOT NULL
        AND to_regclass('public.profile_events_username_once_per_profile') IS NOT NULL",
    )
    .fetch_one(&mut *con)
    .await
    .convert_error()?;
    if !baseline {
        return Err(refuse(
            "requires the pre-P4 FN/P1 schema; no migrations were run",
        ));
    }
    let p4: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM _sqlx_migrations WHERE version >= $1)
        OR to_regclass('public.profile_federation_resets') IS NOT NULL",
    )
    .bind(FIRST_P4)
    .fetch_one(&mut *con)
    .await
    .convert_error()?;
    if p4 {
        return Err(refuse(
            "P4 has already started/applied; offline pre-cutover recovery refused",
        ));
    }
    let valid: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM _sqlx_migrations
                        WHERE version = 20261105000001 AND success)
         AND (SELECT count(*) = 2 FROM pg_index
              WHERE indexrelid IN ('public.profile_events_username_unique'::regclass,
                                   'public.profile_events_username_once_per_profile'::regclass)
                AND indisvalid AND indisunique)",
    )
    .fetch_one(&mut *con)
    .await
    .convert_error()?;
    if !valid {
        return Err(refuse(
            "FN/P1 migration or username uniqueness indexes are not valid",
        ));
    }
    Ok(())
}

#[derive(sqlx::FromRow)]
struct ProfileRow {
    account_id: i64,
    nanoid: String,
    username: Option<String>,
    version: i64,
}

async fn inspect_profile(
    database: &PostgresDatabase,
    con: &mut PostgresConnection,
    id: i64,
) -> error_stack::Result<(ProfileDiagnostic, Option<Profile>), KernelError> {
    let row = sqlx::query_as::<_, ProfileRow>(
        "SELECT account_id, nanoid, username, version FROM profiles WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut **con)
    .await
    .convert_error()?;
    let events = database
        .profile_event_store()
        .find_by_id(con, &EventId::new(id), None)
        .await?;
    let mut profile = None;
    for event in events {
        Profile::apply(&mut profile, event)
            .map_err(|e| e.attach_printable(format!("invalid Profile stream {id}")))?;
    }
    let account_id = match (&profile, &row) {
        (Some(p), _) => *p.account_id().as_ref(),
        (_, Some(r)) => r.account_id,
        _ => return Err(refuse(format!("Profile {id} does not exist"))),
    };
    let account_events = database
        .account_event_store()
        .find_by_id(con, &EventId::new(account_id), None)
        .await?;
    let mut account = None;
    for event in account_events {
        Account::apply(&mut account, event)
            .map_err(|e| e.attach_printable(format!("invalid Account stream {account_id}")))?;
    }
    let account_projection_exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM accounts WHERE id = $1)")
            .bind(account_id)
            .fetch_one(&mut **con)
            .await
            .convert_error()?;
    let problem = match (&profile, &row) {
        (None, _) => {
            Some("Profile has no complete event history; automatic assignment refused".into())
        }
        (Some(_), _) if account.is_none() => {
            Some("owner Account has no complete event history; automatic assignment refused".into())
        }
        (Some(p), Some(r))
            if r.account_id != account_id
                || r.nanoid != p.nanoid().as_ref().as_str()
                || r.username.as_deref() != p.username().as_ref().map(|u| u.as_ref().as_str())
                || r.version != *p.version().as_ref() =>
        {
            Some(
                "Profile projection differs from full history; automatic assignment refused".into(),
            )
        }
        _ => None,
    };
    let diagnostic = ProfileDiagnostic {
        profile_id: id.to_string(),
        nanoid: profile
            .as_ref()
            .map(|p| p.nanoid().as_ref().clone())
            .or_else(|| row.as_ref().map(|r| r.nanoid.clone())),
        username: profile
            .as_ref()
            .and_then(|p| p.username().as_ref().map(|u| u.as_ref().clone()))
            .or_else(|| row.as_ref().and_then(|r| r.username.clone())),
        profile_version: profile.as_ref().map(|p| p.version().as_ref().to_string()),
        account_id: account_id.to_string(),
        account_nanoid: account
            .as_ref()
            .map(|a: &Account| a.nanoid().as_ref().clone()),
        account_version: account.as_ref().map(|a| a.version().as_ref().to_string()),
        account_deactivated: account.as_ref().map(|a| a.deleted_at().is_some()),
        account_status: account.as_ref().map(|a| {
            if a.status().is_banned() {
                "banned"
            } else if a.status().is_suspended() {
                "suspended"
            } else {
                "active"
            }
            .into()
        }),
        profile_projection_exists: row.is_some(),
        account_projection_exists,
        problem,
    };
    Ok((diagnostic, profile))
}

/// Full-history diagnostic in a single consistent, database-enforced read-only snapshot.
pub async fn diagnose(pool: &PgPool) -> error_stack::Result<Diagnosis, KernelError> {
    let database = PostgresDatabase::from_pool(pool.clone());
    let mut tx = database.get_transaction().await?;
    let con = tx.connection();
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut **con)
        .await
        .convert_error()?;
    sqlx::query("SET LOCAL search_path = public")
        .execute(&mut **con)
        .await
        .convert_error()?;
    check_schema(con).await?;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM profiles UNION SELECT id FROM profile_events ORDER BY id",
    )
    .fetch_all(&mut **con)
    .await
    .convert_error()?;
    let mut profiles = Vec::with_capacity(ids.len());
    for id in ids {
        profiles.push(inspect_profile(&database, con, id).await?.0);
    }
    let missing_usernames = profiles.iter().filter(|p| p.username.is_none()).count();
    let problems = profiles.iter().filter(|p| p.problem.is_some()).count();
    tx.commit().await?;
    Ok(Diagnosis {
        profiles,
        missing_usernames,
        problems,
    })
}

fn validate_request(
    request: &UsernameRecoveryRequest,
) -> error_stack::Result<ProfileUsername, KernelError> {
    for (name, value, max) in [
        ("request_id", &request.request_id, 128),
        ("operator", &request.operator, 256),
        ("reason", &request.reason, 2048),
    ] {
        if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
            return Err(refuse(format!(
                "{name} must be nonblank, contain no control characters and be at most {max} bytes"
            )));
        }
    }
    if request.profile_id <= 0 || request.expected_version <= 0 {
        return Err(refuse("profile_id and expected_version must be positive"));
    }
    ProfileUsername::parse(request.username.clone())
}

/// Default dry-run is read-only. Write requires the caller's explicit opt-in and all request
/// fields; the event, existing projection and attributed audit commit or roll back together.
pub async fn set_username(
    pool: &PgPool,
    request: &UsernameRecoveryRequest,
    write: bool,
) -> error_stack::Result<RecoveryResult, KernelError> {
    let username = validate_request(request)?;
    let database = PostgresDatabase::from_pool(pool.clone());
    let mut tx = database.get_transaction().await?;
    let con = tx.connection();
    if !write {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
            .execute(&mut **con)
            .await
            .convert_error()?;
    }
    sqlx::query("SET LOCAL search_path = public")
        .execute(&mut **con)
        .await
        .convert_error()?;
    sqlx::query("SET LOCAL lock_timeout = '5s'")
        .execute(&mut **con)
        .await
        .convert_error()?;
    check_schema(con).await?;
    if write {
        // Offline-only: exclude event/API/projector writers and other recovery invocations.
        sqlx::query(
            "LOCK TABLE _sqlx_migrations, account_events, accounts, profile_events, profiles
             IN SHARE ROW EXCLUSIVE MODE",
        )
        .execute(&mut **con)
        .await
        .convert_error()?;
        check_schema(con).await?;
    }
    let has_audit: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.profile_cutover_recovery_audit') IS NOT NULL",
    )
    .fetch_one(&mut **con)
    .await
    .convert_error()?;
    if has_audit {
        if write {
            sqlx::query("LOCK TABLE profile_cutover_recovery_audit IN SHARE ROW EXCLUSIVE MODE")
                .execute(&mut **con)
                .await
                .convert_error()?;
        }
        let previous: Option<(serde_json::Value, serde_json::Value)> = sqlx::query_as(
            "SELECT request, result FROM profile_cutover_recovery_audit WHERE request_id = $1",
        )
        .bind(&request.request_id)
        .fetch_optional(&mut **con)
        .await
        .convert_error()?;
        if let Some((old_request, old_result)) = previous {
            let old_request: UsernameRecoveryRequest =
                serde_json::from_value(old_request).convert_error()?;
            if &old_request != request {
                return Err(refuse(
                    "request_id was already used with different arguments",
                ));
            }
            let mut result: RecoveryResult = serde_json::from_value(old_result).convert_error()?;
            result.status = "already_applied".into();
            tx.commit().await?;
            return Ok(result);
        }
    }
    let (diagnostic, profile) = inspect_profile(&database, con, request.profile_id).await?;
    if let Some(problem) = &diagnostic.problem {
        return Err(refuse(problem.clone()));
    }
    let profile = profile.ok_or_else(|| refuse("Profile event history required"))?;
    if *profile.version().as_ref() != request.expected_version {
        return Err(refuse(
            "Profile version changed; re-diagnose and review the current owner",
        ));
    }
    if profile.username().is_some() {
        return Err(refuse("Profile already has an immutable username"));
    }
    let taken: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM profile_events WHERE data ->> 'username' = $1)
        OR EXISTS (SELECT 1 FROM profiles WHERE username = $1)",
    )
    .bind(&request.username)
    .fetch_one(&mut **con)
    .await
    .convert_error()?;
    if taken {
        return Err(refuse(
            "username is already reserved (including dormant Profiles)",
        ));
    }
    let mut result = RecoveryResult {
        status: "dry_run".into(),
        profile: diagnostic,
        new_version: None,
    };
    if write {
        sqlx::query(AUDIT_DDL)
            .execute(&mut **con)
            .await
            .convert_error()?;
        let event = database
            .profile_event_store()
            .persist_and_transform(
                con,
                Profile::set_username(ProfileId::new(request.profile_id), username),
            )
            .await?;
        let new_version = *event.version.as_ref();
        if new_version <= request.expected_version {
            return Err(refuse(
                "generated event version is not newer; check the local clock/WORKER_ID",
            ));
        }
        if result.profile.profile_projection_exists {
            let updated = sqlx::query(
                "UPDATE profiles SET username = $1, version = $2
                 WHERE id = $3 AND version = $4 AND username IS NULL",
            )
            .bind(&request.username)
            .bind(new_version)
            .bind(request.profile_id)
            .bind(request.expected_version)
            .execute(&mut **con)
            .await
            .convert_error()?
            .rows_affected();
            if updated != 1 {
                return Err(refuse(
                    "Profile projection changed; transaction rolled back",
                ));
            }
        }
        result.status = "applied".into();
        result.new_version = Some(new_version.to_string());
        sqlx::query(
            "INSERT INTO profile_cutover_recovery_audit (request_id, request, result)
             VALUES ($1, $2, $3)",
        )
        .bind(&request.request_id)
        .bind(serde_json::to_value(request).convert_error()?)
        .bind(serde_json::to_value(&result).convert_error()?)
        .execute(&mut **con)
        .await
        .convert_error()?;
    }
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
mod tests;
