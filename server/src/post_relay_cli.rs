//! Dispatch before normal initialization: no migrations, Ory, listener or worker.
use crate::error::StackTrace;
use error_stack::Report;
use kernel::interfaces::post_relay::CapacityReport;
use kernel::KernelError;
use std::collections::BTreeMap;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};
const HELP:&str="Emumet server (no arguments: normal server startup)\n\nOperator commands (no HTTP management API):\n  server capacity report --free-bytes N --total-bytes N --sampled-at RFC3339\n\nDATABASE_URL is required. Reports the most constrained DB data/WAL/tablespace or managed-DB\nvolume, NOT container disk. The deployment monitor supplies a sample at least every 30s.\nNo migrations are applied; mismatched schemas are refused. Unknown or >90s-old capacity\nblocks new relay admission. Stop below 10GiB or 10%; recover at 12GiB AND 12%.\nT1 supplies the compose/E2E sampler. See docs/post-relay-storage.md.\n";
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve,
    Help,
    Report {
        free: i64,
        total: i64,
        sampled_at: OffsetDateTime,
    },
}
fn parse(args: impl IntoIterator<Item = String>) -> Result<Command, &'static str> {
    let args: Vec<_> = args.into_iter().collect();
    if args.is_empty() {
        return Ok(Command::Serve);
    }
    if args == ["--help"]
        || args == ["-h"]
        || args == ["capacity", "--help"]
        || args == ["capacity", "report", "--help"]
    {
        return Ok(Command::Help);
    }
    if args.len() != 8 || args[0] != "capacity" || args[1] != "report" {
        return Err("invalid server command; see server --help");
    }
    let mut flags = BTreeMap::new();
    for pair in args[2..].as_chunks::<2>().0 {
        if !matches!(
            pair[0].as_str(),
            "--free-bytes" | "--total-bytes" | "--sampled-at"
        ) || flags.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err("invalid capacity report flags; see server capacity report --help");
        }
    }
    let free = flags["--free-bytes"]
        .parse()
        .map_err(|_| "invalid free bytes")?;
    let total = flags["--total-bytes"]
        .parse()
        .map_err(|_| "invalid total bytes")?;
    let sampled_at = OffsetDateTime::parse(flags["--sampled-at"], &Rfc3339)
        .map_err(|_| "invalid sample time (RFC3339 required)")?;
    if free < 0 || total <= 0 || free > total {
        return Err("invalid capacity dimensions");
    }
    Ok(Command::Report {
        free,
        total,
        sampled_at,
    })
}
pub async fn dispatch(args: impl IntoIterator<Item = String>) -> Result<bool, StackTrace> {
    let command =
        parse(args).map_err(|e| Report::new(KernelError::Validation).attach_printable(e))?;
    match command {
        Command::Serve => Ok(false),
        Command::Help => {
            print!("{HELP}");
            Ok(true)
        }
        Command::Report {
            free,
            total,
            sampled_at,
        } => {
            let url = std::env::var("DATABASE_URL").map_err(|_| {
                Report::new(KernelError::Validation)
                    .attach_printable("capacity report requires DATABASE_URL")
            })?;
            // Do not print DB errors or connection URLs (credentials), payloads or environment values.
            let result=driver::post_relay::publish_capacity(&url,&CapacityReport{free_bytes:free,total_bytes:total,sampled_at}).await.map_err(|_|Report::new(KernelError::Rejected).attach_printable("capacity report refused: check DB reachability, exact schema, dimensions and a fresh, monotonically newer sample; no migrations were applied"))?;
            println!(
                "{}",
                serde_json::to_string(&result).map_err(|_| Report::new(KernelError::Internal))?
            );
            Ok(true)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn capacity_command_help_and_strict_parse() {
        assert_eq!(parse(args("")).unwrap(), Command::Serve);
        for s in ["--help", "capacity --help", "capacity report --help"] {
            assert_eq!(parse(args(s)).unwrap(), Command::Help);
        }
        assert!(matches!(parse(args("capacity report --free-bytes 12 --total-bytes 100 --sampled-at 2026-11-22T00:00:00Z")).unwrap(),Command::Report{free:12,total:100,..}));
        for s in [
            "relay send",
            "capacity report",
            "capacity report --free-bytes 12 --free-bytes 100 --sampled-at 2026-11-22T00:00:00Z",
            "capacity report --free-bytes -1 --total-bytes 100 --sampled-at 2026-11-22T00:00:00Z",
            "capacity report --free-bytes 101 --total-bytes 100 --sampled-at 2026-11-22T00:00:00Z",
        ] {
            assert!(parse(args(s)).is_err(), "{s}");
        }
    }
}
