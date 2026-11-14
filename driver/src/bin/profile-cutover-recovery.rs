use driver::profile_cutover_recovery::{diagnose, set_username, UsernameRecoveryRequest};
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use std::process::ExitCode;

const HELP: &str = "\
Offline pre-P4 Profile recovery (never starts the server or runs migrations).
Stop every old/new server and worker and take a backup before --write.

DATABASE_URL=... profile-cutover-recovery diagnose
DATABASE_URL=... profile-cutover-recovery set-username
  --profile-id ID --expected-version VERSION --username NAME
  --request-id UNIQUE-ID --operator OPERATOR --reason REASON [--write]

set-username defaults to a read-only dry-run. Reuse the EXACT request on retry.
Username is permanent; obtain the owner's agreement before assigning it.
WORKER_ID: Snowflake worker id 0..1023 (default 0; all other writers must be stopped).
JSON results go to stdout; errors go to stderr and return nonzero. Retain both.
";

#[derive(Debug)]
enum Command {
    Diagnose,
    SetUsername(UsernameRecoveryRequest, bool),
    Help,
}

fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Err(HELP.into());
    };
    if matches!(command.as_str(), "--help" | "-h") && args.next().is_none() {
        return Ok(Command::Help);
    }
    if command == "diagnose" && args.next().is_none() {
        return Ok(Command::Diagnose);
    }
    if command != "set-username" {
        return Err("expected diagnose or set-username; see --help".into());
    }
    let mut flags = BTreeMap::new();
    let mut write = false;
    while let Some(flag) = args.next() {
        if flag == "--write" && !write {
            write = true;
            continue;
        }
        if !matches!(
            flag.as_str(),
            "--profile-id"
                | "--expected-version"
                | "--username"
                | "--request-id"
                | "--operator"
                | "--reason"
        ) {
            return Err(format!("unknown or repeated flag: {flag}"));
        }
        let value = args
            .next()
            .filter(|v| !v.starts_with("--"))
            .ok_or_else(|| format!("missing value for {flag}"))?;
        if flags.insert(flag.clone(), value).is_some() {
            return Err(format!("repeated flag: {flag}"));
        }
    }
    let mut required = |name: &str| {
        flags
            .remove(name)
            .ok_or_else(|| format!("{name} is required, also for dry-run"))
    };
    Ok(Command::SetUsername(
        UsernameRecoveryRequest {
            profile_id: required("--profile-id")?
                .parse()
                .map_err(|_| "invalid --profile-id")?,
            expected_version: required("--expected-version")?
                .parse()
                .map_err(|_| "invalid --expected-version")?,
            username: required("--username")?,
            request_id: required("--request-id")?,
            operator: required("--operator")?,
            reason: required("--reason")?,
        },
        write,
    ))
}

async fn execute(command: Command) -> Result<(), String> {
    if matches!(command, Command::Help) {
        print!("{HELP}");
        return Ok(());
    }
    // Do not use AppModule or PostgresDatabase::new: either would trigger P4 migrations.
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(&url)
        .await
        .map_err(|_| "database connection failed (check DATABASE_URL and DB permissions)")?;
    let output = match command {
        Command::Diagnose => {
            serde_json::to_value(diagnose(&pool).await.map_err(|e| format!("{e:?}"))?)
        }
        Command::SetUsername(request, write) => {
            if write {
                let worker_id = std::env::var("WORKER_ID")
                    .unwrap_or_else(|_| "0".into())
                    .parse::<u64>()
                    .ok()
                    .filter(|id| *id <= 1023)
                    .ok_or("WORKER_ID must be 0..1023")?;
                kernel::init_generator(worker_id);
            }
            let result = set_username(&pool, &request, write).await.map_err(|e| {
                format!(
                    "request_id={:?} operator={:?}: {e:?}",
                    request.request_id, request.operator
                )
            })?;
            serde_json::to_value(result)
        }
        Command::Help => unreachable!(),
    }
    .map_err(|_| "could not serialize the result")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&output).map_err(|_| "could not serialize the result")?
    );
    pool.close().await;
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let result = match parse(std::env::args().skip(1)) {
        Ok(command) => execute(command).await,
        Err(error) => Err(error),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("profile-cutover-recovery refused: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(extra: &[&str]) -> Vec<String> {
        [
            "set-username",
            "--profile-id",
            "10",
            "--expected-version",
            "20",
            "--username",
            "alice",
            "--request-id",
            "ticket-1",
            "--operator",
            "operator",
            "--reason",
            "owner approved",
        ]
        .into_iter()
        .chain(extra.iter().copied())
        .map(String::from)
        .collect()
    }

    #[test]
    fn write_is_explicit_and_bad_arguments_fail_closed() {
        assert!(matches!(
            parse(args(&[])).unwrap(),
            Command::SetUsername(_, false)
        ));
        assert!(matches!(
            parse(args(&["--write"])).unwrap(),
            Command::SetUsername(_, true)
        ));
        for extra in [
            vec!["--write", "--write"],
            vec!["--username", "bob"],
            vec!["--unknown"],
            vec!["--reason"],
        ] {
            assert!(parse(args(&extra)).is_err());
        }
        assert!(parse(["diagnose", "--write"].map(String::from)).is_err());
        assert!(parse(["set-username", "--write"].map(String::from)).is_err());
        assert!(matches!(parse(["--help".into()]).unwrap(), Command::Help));
    }
}
