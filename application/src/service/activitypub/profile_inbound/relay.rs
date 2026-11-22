//! Raw inbox admission joins relation/Undo work and Q1 storage on one connection.
use super::*;
use crate::inbox_reading::InboxReading;
use base64::{engine::general_purpose::STANDARD, Engine};
use kernel::interfaces::http_signing::{SignatureEvidence, SignatureScheme};
use kernel::interfaces::post_relay::{
    Admission, DependOnPostRelayStore, LimitScope, PostRelayStore, RefusalReason, RelayAdmission,
    RelayRecipient,
};
use serde_json::{json, Value};

/// No Debug: body and header evidence must not reach diagnostics.
pub struct ProfileRelayInboxDto {
    pub profile_nanoid: String,
    pub reading: InboxReading,
    pub raw_body: Vec<u8>,
    pub verification: SignatureEvidence,
    pub request_target: String,
    pub evidence: Value,
    pub enabled: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct InboxOverload {
    pub status: u16,
    pub retry_after_secs: u32,
    pub reason: RefusalReason,
}
pub trait ProfileRelayInboxUseCase: ProfileInboxUseCase + DependOnPostRelayStore {
    fn admit_profile_inbox(
        &self,
        dto: ProfileRelayInboxDto,
    ) -> impl Future<Output = error_stack::Result<ProfileInboxOutcome, KernelError>> + Send {
        admit(self, dto)
    }
}
impl<T: ProfileInboxUseCase + DependOnPostRelayStore> ProfileRelayInboxUseCase for T {}

async fn certain<T: ProfileRelayInboxUseCase>(
    deps: &T,
    c: &mut Conn<T>,
    receiver: &ProfileId,
    r: &Value,
) -> error_stack::Result<bool, KernelError> {
    if InboxReading::embedded_relation(r) {
        return Ok(true);
    }
    let Some(id) = reference_id(r).and_then(|id| GraphActivityId::new(id).ok()) else {
        return Ok(false);
    };
    // A real received Follow/Block is evidence; an UndoFollow marker is NOT.
    if deps
        .inbound_activity_repository()
        .has_observed_relation(c, receiver, &id)
        .await?
    {
        return Ok(true);
    }
    // Local outbound generations are real Follow operations, never early-Undo tombstones.
    Ok(deps
        .profile_follow_repository()
        .find_by_follower_activity(c, &ProfileGraphActor::Local(receiver.clone()), &id)
        .await?
        .is_some())
}
fn envelope(
    dto: &ProfileRelayInboxDto,
    base: &str,
    profile_actor: &str,
    receipt: &str,
    at: OffsetDateTime,
    recipient: &RelayRecipient,
) -> error_stack::Result<Vec<u8>, KernelError> {
    let timestamp = |at: OffsetDateTime| {
        at.format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| Report::new(KernelError::Internal))
    };
    let value = json!({
        "version":1,"receipt_id":receipt,"received_at":timestamp(at)?,
        "sender":{"origin":base,"instance_actor":format!("{base}/ap/instance")},
        "recipient":{"origin":recipient.origin,"profile_actor":profile_actor,"link_id":recipient.link_id,"link_generation":recipient.generation},
        "activity":{"id":dto.reading.activity_id,"actor":dto.reading.actor,"body_encoding":"base64","raw_body":STANDARD.encode(&dto.raw_body)},
        "verification":{"result":"verified","scheme":match dto.verification.scheme {SignatureScheme::Cavage=>"cavage",SignatureScheme::Rfc9421=>"rfc9421"},"key_id":dto.verification.key_id,"signer_actor":dto.verification.owner,"verified_at":timestamp(dto.verification.verified_at)?},
        "original_request":{"method":"POST","request_target":dto.request_target,"signature_headers":dto.evidence["signature_headers"]},
        "evidence_complete":dto.evidence["evidence_complete"],"omitted_header_names":dto.evidence["omitted_header_names"],"omitted_header_count":dto.evidence["omitted_header_count"],
    });
    serde_json::to_vec(&value).map_err(|_| Report::new(KernelError::Internal))
}
async fn admit<T: ProfileRelayInboxUseCase>(
    module: &T,
    dto: ProfileRelayInboxDto,
) -> error_stack::Result<ProfileInboxOutcome, KernelError> {
    if !crate::inbox_reading::is_remote_url(&dto.verification.owner)
        || !crate::inbox_reading::is_remote_url(&dto.verification.key_id)
        || !same_id(&dto.verification.owner, &dto.reading.actor)
    {
        return Err(Report::new(KernelError::PermissionDenied));
    }
    let mut activity = dto.reading.relation_activity();
    // Follow is excluded and keeps its existing network-before-transaction relation path.
    if dto.reading.relation.as_deref() == Some("Follow") {
        return handle(
            module,
            ProfileInboxDto {
                profile_nanoid: dto.profile_nanoid,
                signer_actor_url: dto.verification.owner,
                activity,
            },
        )
        .await;
    }
    let (receiver, local_actor_url) = resolve_receiver(module, &dto.profile_nanoid).await?;
    // Forwarding accepts the contract URL_MAX=2048; the legacy graph/schema only 1024.
    // Never turn that legacy representation limit into an admission rejection. Relation
    // authorization uses the verified owner (not the slash-equivalent body actor).
    let context = GraphActorUrl::new(dto.verification.owner.clone())
        .ok()
        .map(|signer| Context {
            receiver: receiver.clone(),
            local_actor_url: local_actor_url.clone(),
            signer,
        });
    let own_id_fits_graph =
        activity.id.is_empty() || GraphActivityId::new(activity.id.clone()).is_ok();
    let is_undo = dto.reading.relation.as_deref() == Some("Undo");
    if is_undo && !own_id_fits_graph {
        // An unrepresentable outer receipt must not discard a short Follow cancellation.
        // apply_undo permits no outer id and still records the receiver-scoped guard.
        activity.id.clear();
    }
    let deps = module.clone();
    module
        .transaction_manager()
        .transaction(move |c| {
            Box::pin(async move {
                // Takes Account/Profile/relationship gates before the link row (L1/Q1 order).
                let recipient = deps
                    .post_relay_store()
                    .prepare_admission(c, &receiver)
                    .await?;
                let mut exclude = matches!(dto.reading.relation.as_deref(), Some("Block"));
                if matches!(
                    dto.reading.relation.as_deref(),
                    Some("Undo" | "Accept" | "Reject")
                ) {
                    let refs = dto.reading.references();
                    exclude = !refs.is_empty();
                    for r in refs {
                        if !certain(&deps, c, &receiver, r).await? {
                            exclude = false;
                        }
                    }
                }
                let mut outcome = ignored(ProfileInboxIgnored::UnsupportedActivity);
                match (context.as_ref(), dto.reading.relation.as_deref()) {
                    (Some(ctx), Some("Block")) => {
                        if let Some(id) = dto
                            .reading
                            .activity_id
                            .as_ref()
                            .and_then(|id| GraphActivityId::new(id.clone()).ok())
                        {
                            deps.inbound_activity_repository()
                                .record(
                                    c,
                                    &inbound_record(ctx, &id, "Block", OffsetDateTime::now_utc())?,
                                )
                                .await?;
                        }
                    }
                    (Some(ctx), Some("Undo" | "Accept" | "Reject")) => {
                        // An array/missing object is not a unique legacy relation operation.
                        let process = match follow_object(activity.object.as_ref()) {
                            FollowObject::Embedded { id, .. } => {
                                id.is_none_or(|id| GraphActivityId::new(id).is_ok())
                            }
                            FollowObject::Id(id) => GraphActivityId::new(id).is_ok(),
                            _ => false,
                        };
                        if process && (own_id_fits_graph || is_undo) {
                            let base = deps.public_base_url().as_str().trim_end_matches('/');
                            if ctx.signer.as_str().starts_with(&format!("{base}/ap/")) {
                                return Err(rejected(
                                    "local actor cannot deliver a relation over HTTP",
                                ));
                            }
                            let context = Context {
                                receiver: ctx.receiver.clone(),
                                local_actor_url: ctx.local_actor_url.clone(),
                                signer: ctx.signer.clone(),
                            };
                            outcome = match dto.reading.relation.as_deref() {
                                Some("Undo") => apply_undo(&deps, c, context, activity).await?,
                                Some("Accept" | "Reject") if !activity.id.is_empty() => {
                                    apply_response(
                                        &deps,
                                        c,
                                        context,
                                        activity,
                                        if dto.reading.relation.as_deref() == Some("Accept") {
                                            Response::Accept
                                        } else {
                                            Response::Reject
                                        },
                                    )
                                    .await?
                                }
                                _ if exclude => {
                                    return Err(rejected(
                                        "known relation response requires an activity id",
                                    ))
                                }
                                _ => outcome,
                            };
                        }
                    }
                    _ => {}
                }
                if exclude || !dto.enabled {
                    return Ok(outcome);
                }
                // Duplicate only prevents relation mutation, never admission. Failure rolls back
                // receipt/Undo work. A lost 202 may duplicate, but cannot silently lose the relay.
                let at = deps.post_relay_store().admission_time(c).await?;
                let receipt = Nanoid::<Profile>::default().as_ref().to_string();
                let bytes = match &recipient {
                    Some(r) => envelope(
                        &dto,
                        deps.public_base_url().as_str().trim_end_matches('/'),
                        &local_actor_url,
                        &receipt,
                        at,
                        r,
                    )?,
                    None => vec![b' '],
                };
                match deps
                    .post_relay_store()
                    .enqueue(
                        c,
                        &RelayAdmission {
                            profile_id: receiver,
                            receipt,
                            received_at: at,
                            recipient,
                            envelope: bytes,
                        },
                    )
                    .await?
                {
                    Admission::Queued | Admission::Unlinked => Ok(outcome),
                    Admission::Limited {
                        scope,
                        retry_after_secs,
                        reason,
                    } => Err(Report::new(KernelError::Rejected).attach(InboxOverload {
                        status: if scope == LimitScope::Profile {
                            429
                        } else {
                            503
                        },
                        retry_after_secs,
                        reason,
                    })),
                    Admission::Unavailable => {
                        Err(Report::new(KernelError::Rejected).attach(InboxOverload {
                            status: 503,
                            retry_after_secs: 1,
                            reason: RefusalReason::Capacity,
                        }))
                    }
                }
            })
        })
        .await
}
