//! Operator surface dispatched before logging, Snowflake initialization or AppModule.
use crate::{error::StackTrace, post_relay_worker::PostRelayModule};
use application::{
    instance_identity::InstanceKeyCache, service::post_relay::delivery::RelayDelivery,
};
use driver::{
    crypto::FilePasswordProvider, home_origin::HomeOriginAllowlist,
    post_relay_delivery::HttpPostRelayTransport,
};
use error_stack::Report;
use kernel::interfaces::{
    config::PublicBaseUrl,
    database::{DependOnTransactionManager, TransactionManager},
    post_relay::*,
};
use kernel::{prelude::entity::ProfileId, KernelError};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
pub(crate) const HELP: &str = "
Post-relay recovery (release binary; no Cargo/source, migrations, Ory, S3 or listener):
  server post-relay preview --action redeliver|discard --profile-id ID [--receipt ID]
      --operation UUID --operator NAME --reason TEXT
  server post-relay redeliver|discard --profile-id ID [--receipt ID]
      --operation UUID --operator NAME --reason TEXT --confirm COUNT:BYTES:BOUNDARY
  server post-relay status --operation UUID
Each invocation stages/discards at most 128 selected envelopes. Repeat with the SAME operation
and confirmation to advance a frozen selection, not newly arrived failures. Preview first.
DATABASE_URL required; exact schema only. UUID operation IDs and PostgreSQL fencing sequences
are used exclusively: no Snowflake IDs, WORKER_ID or running-server ID collision.
Redeliver also needs PUBLIC_BASE_URL, master-key-password and allowed origins. Setting
POST_RELAY_SENDER_ENABLED=false refuses redelivery. Already-authorized bytes cannot be revoked.
See docs/post-relay-operations.md.
";
#[derive(Debug, PartialEq, Eq)]
enum Action {
    Preview(OperatorAction),
    Redeliver,
    Discard,
    Status,
    Help,
}
struct Command {
    action: Action,
    flags: BTreeMap<String, String>,
}
fn invalid() -> Report<KernelError> {
    Report::new(KernelError::Validation)
        .attach_printable("invalid post-relay command; see server post-relay --help")
}
fn parse(args: Vec<String>) -> Result<Command, Report<KernelError>> {
    if args.len() == 1 || args.get(1).is_some_and(|a| a == "--help") {
        return Ok(Command {
            action: Action::Help,
            flags: BTreeMap::new(),
        });
    }
    let (pairs, rest) = args[2..].as_chunks::<2>();
    if !rest.is_empty() {
        return Err(invalid());
    }
    let mut flags = BTreeMap::new();
    for [k, v] in pairs {
        if !matches!(
            k.as_str(),
            "--action"
                | "--profile-id"
                | "--receipt"
                | "--operation"
                | "--operator"
                | "--reason"
                | "--confirm"
        ) || v.is_empty()
            || flags.insert(k.clone(), v.clone()).is_some()
        {
            return Err(invalid());
        }
    }
    let action = match args[1].as_str() {
        "status" => Action::Status,
        "preview" => Action::Preview(match flags.get("--action").map(String::as_str) {
            Some("redeliver") => OperatorAction::Redeliver,
            Some("discard") => OperatorAction::Discard,
            _ => return Err(invalid()),
        }),
        "redeliver" => Action::Redeliver,
        "discard" => Action::Discard,
        _ => return Err(invalid()),
    };
    if flags
        .get("--operation")
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .is_none()
    {
        return Err(invalid());
    }
    if action == Action::Status {
        if flags.len() != 1 {
            return Err(invalid());
        }
    } else {
        for k in ["--profile-id", "--operator", "--reason"] {
            if !flags.contains_key(k) {
                return Err(invalid());
            }
        }
        if flags["--profile-id"].parse::<i64>().is_err() {
            return Err(invalid());
        }
        if matches!(action, Action::Discard | Action::Redeliver)
            && (!flags.contains_key("--confirm") || flags.contains_key("--action"))
        {
            return Err(invalid());
        }
        if matches!(action, Action::Preview(_)) && flags.contains_key("--confirm") {
            return Err(invalid());
        }
    }
    Ok(Command { action, flags })
}
fn confirmation(p: &OperatorPreview) -> String {
    format!("{}:{}:{}", p.count, p.bytes, p.boundary)
}
pub(crate) async fn dispatch(args: Vec<String>) -> Result<bool, StackTrace> {
    let c = parse(args)?;
    if c.action == Action::Help {
        print!("{HELP}");
        return Ok(true);
    }
    let url = std::env::var("DATABASE_URL").map_err(|_| invalid())?;
    // Sanitize all operational errors; SQL errors can contain credentials/remote/payload details.
    execute(c,&url).await.map_err(|_|Report::new(KernelError::Rejected).attach_printable("post-relay operation refused; check schema, eligibility, confirmation, policy, expiry and database availability; no migrations applied"))?;
    Ok(true)
}
async fn execute(c: Command, url: &str) -> Result<(), StackTrace> {
    let db = driver::post_relay::connect_operator(url).await?;
    if c.action == Action::Status {
        println!(
            "{}",
            driver::post_relay::operation_status(&db, &c.flags["--operation"]).await?
        );
        return Ok(());
    }
    let action = match c.action {
        Action::Preview(a) => a,
        Action::Redeliver => OperatorAction::Redeliver,
        _ => OperatorAction::Discard,
    };
    if c.action == Action::Redeliver && !crate::post_relay_worker::enabled()? {
        return Err(Report::new(KernelError::Rejected).into());
    }
    let request = OperatorRequest {
        id: c.flags["--operation"].clone(),
        profile_id: ProfileId::new(
            c.flags["--profile-id"]
                .parse::<i64>()
                .map_err(|_| invalid())?,
        ),
        receipt: c.flags.get("--receipt").cloned(),
        action,
        operator: c.flags["--operator"].clone(),
        reason: c.flags["--reason"].clone(),
    };
    if matches!(c.action, Action::Preview(_)) {
        let d = db.clone();
        let preview = db
            .transaction_manager()
            .transaction(move |con| {
                Box::pin(async move { d.post_relay_store().operator_preview(con, &request).await })
            })
            .await?;
        println!(
            "{}",
            serde_json::json!({"state":"quarantined","count":preview.count,"bytes":preview.bytes,"earliest_expiry":preview.earliest_expiry.map(|v|v.to_string()),"confirmation":confirmation(&preview),"max_batch":128})
        );
        return Ok(());
    }
    let module = if action == OperatorAction::Redeliver {
        Some(PostRelayModule {
            db: db.clone(),
            encryptor: driver::crypto::Argon2Encryptor::default(),
            cache: Arc::new(InstanceKeyCache::default()),
            password: FilePasswordProvider::new(),
            origin: PublicBaseUrl::new(dotenvy::var("PUBLIC_BASE_URL").map_err(|_| invalid())?),
            transport: HttpPostRelayTransport::new(HomeOriginAllowlist::from_env()?),
        })
    } else {
        None
    };
    if let Some(module) = &module {
        use application::instance_identity::UnlockInstanceSigningKeyUseCase;
        module.unlock_instance_signing_key().await?;
    }
    let policy = module.as_ref().map(|m| m.transport.clone());
    let d = db.clone();
    let req = request.clone();
    let confirm = c.flags["--confirm"].clone();
    let batch = db
        .transaction_manager()
        .transaction(move |con| {
            Box::pin(async move {
                if let Some(policy) = policy {
                    let recipient = d
                        .post_relay_store()
                        .prepare_admission(con, &req.profile_id)
                        .await?
                        .ok_or_else(|| Report::new(KernelError::PermissionDenied))?;
                    if !policy.origin_allowed(&recipient.origin) {
                        return Err(Report::new(KernelError::PermissionDenied));
                    }
                }
                let preview = d.post_relay_store().begin_operator(con, &req).await?;
                // begin_operator returns the original frozen counts on resume. Reject stale preview in
                // the SAME transaction, before any staging/deletion/audit commits.
                if confirmation(&preview) != confirm {
                    return Err(Report::new(KernelError::Concurrency));
                }
                if action == OperatorAction::Discard {
                    d.post_relay_store()
                        .operator_discard(con, &req.id, true)
                        .await
                } else {
                    d.post_relay_store().operator_redeliver(con, &req.id).await
                }
            })
        })
        .await?;
    println!(
        "{}",
        serde_json::json!({"batch_selected":batch.selected,"batch_changed":batch.changed,"selection_complete":batch.complete})
    );
    if let Some(module) = module {
        let worker = Arc::new(RelayDelivery::new(Arc::new(module)));
        // At most one batch; other server processes may win its fenced claims. They share the
        // same 16/4 slots. Interruption leaves staged jobs recoverable and does not resend an
        // already-attempted envelope within this operation.
        let operation = request.id.clone();
        let work = async {
            loop {
                let status = driver::post_relay::operation_status(&db, &operation).await?;
                if status["pending"].as_i64() == Some(0) {
                    break;
                }
                let leases = worker.claim_operation(16, Some(operation.clone())).await?;
                let mut tasks = tokio::task::JoinSet::new();
                for l in leases {
                    let w = worker.clone();
                    tasks.spawn(async move { w.deliver(l).await });
                }
                while let Some(r) = tasks.join_next().await {
                    r.map_err(|_| Report::new(KernelError::Internal))??;
                }
                worker.maintain().await?;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            Ok::<(), StackTrace>(())
        };
        tokio::time::timeout(Duration::from_secs(400), work)
            .await
            .map_err(|_| Report::new(KernelError::Rejected))??;
    }
    println!(
        "{}",
        driver::post_relay::operation_status(&db, &request.id).await?
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }
    #[test]
    fn operator_parse_requires_confirmation_and_bounded_identity() {
        let base="post-relay discard --profile-id 123 --operation 14a75dc9-81ad-4e41-b9ed-3e826f62f752 --operator ops --reason incident";
        assert!(parse(args(base)).is_err());
        assert!(parse(args(&format!("{base} --confirm 1:100:4"))).is_ok());
        assert!(parse(args("post-relay status --operation nonsense")).is_err());
        assert!(parse(args("post-relay --help")).is_ok());
    }
}
