mod id;

pub use self::id::*;

use super::{
    AccountId, CommandEnvelope, EventEnvelope, EventId, EventVersion, ExpectedVersion, Nanoid,
    ProfileId,
};
use crate::event::EventApplier;
use crate::KernelError;
use destructure::Destructure;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{Nameln, Newln, References};

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Destructure, Serialize, Deserialize,
)]
pub struct ProfileTransferRequest {
    id: ProfileTransferRequestId,
    profile_id: ProfileId,
    from_account_id: AccountId,
    to_org_account_id: AccountId,
    status: ProfileTransferStatus,
    version: EventVersion<ProfileTransferRequest>,
    nanoid: Nanoid<ProfileTransferRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileTransferStatus {
    Pending,
    Accepted,
    Rejected,
    Cancelled,
}

impl ProfileTransferStatus {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending)
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[vodca(prefix = "profile_transfer_request", snake_case)]
pub enum ProfileTransferRequestEvent {
    Requested {
        profile_id: ProfileId,
        from_account_id: AccountId,
        to_org_account_id: AccountId,
        nanoid: Nanoid<ProfileTransferRequest>,
    },
    Accepted,
    Rejected,
    Cancelled,
}

impl ProfileTransferRequest {
    pub fn request(
        id: ProfileTransferRequestId,
        profile_id: ProfileId,
        from_account_id: AccountId,
        to_org_account_id: AccountId,
        nanoid: Nanoid<ProfileTransferRequest>,
    ) -> CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest> {
        let event = ProfileTransferRequestEvent::Requested {
            profile_id,
            from_account_id,
            to_org_account_id,
            nanoid,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(ExpectedVersion::Nothing),
        )
    }

    pub fn accept(
        id: ProfileTransferRequestId,
        current_version: EventVersion<ProfileTransferRequest>,
    ) -> CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest> {
        let event = ProfileTransferRequestEvent::Accepted;
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(ExpectedVersion::At(current_version)),
        )
    }

    pub fn reject(
        id: ProfileTransferRequestId,
        current_version: EventVersion<ProfileTransferRequest>,
    ) -> CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest> {
        let event = ProfileTransferRequestEvent::Rejected;
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(ExpectedVersion::At(current_version)),
        )
    }

    pub fn cancel(
        id: ProfileTransferRequestId,
        current_version: EventVersion<ProfileTransferRequest>,
    ) -> CommandEnvelope<ProfileTransferRequestEvent, ProfileTransferRequest> {
        let event = ProfileTransferRequestEvent::Cancelled;
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(ExpectedVersion::At(current_version)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reconstitute(
        id: ProfileTransferRequestId,
        profile_id: ProfileId,
        from_account_id: AccountId,
        to_org_account_id: AccountId,
        status: ProfileTransferStatus,
        version: EventVersion<ProfileTransferRequest>,
        nanoid: Nanoid<ProfileTransferRequest>,
    ) -> Self {
        Self {
            id,
            profile_id,
            from_account_id,
            to_org_account_id,
            status,
            version,
            nanoid,
        }
    }
}

impl EventApplier for ProfileTransferRequest {
    type Event = ProfileTransferRequestEvent;
    const ENTITY_NAME: &'static str = "ProfileTransferRequest";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError> {
        match event.event {
            ProfileTransferRequestEvent::Requested {
                profile_id,
                from_account_id,
                to_org_account_id,
                nanoid,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(ProfileTransferRequest {
                    id: ProfileTransferRequestId::new(event.id),
                    profile_id,
                    from_account_id,
                    to_org_account_id,
                    status: ProfileTransferStatus::Pending,
                    version: event.version,
                    nanoid,
                });
            }
            ProfileTransferRequestEvent::Accepted
            | ProfileTransferRequestEvent::Rejected
            | ProfileTransferRequestEvent::Cancelled => {
                if let Some(request) = entity {
                    if !request.status.is_pending() {
                        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
                            "Profile transfer request is already {}",
                            status_name(&request.status)
                        )));
                    }
                    request.status = match event.event {
                        ProfileTransferRequestEvent::Accepted => ProfileTransferStatus::Accepted,
                        ProfileTransferRequestEvent::Rejected => ProfileTransferStatus::Rejected,
                        ProfileTransferRequestEvent::Cancelled => ProfileTransferStatus::Cancelled,
                        ProfileTransferRequestEvent::Requested { .. } => unreachable!(),
                    };
                    request.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
        }
        Ok(())
    }
}

fn status_name(status: &ProfileTransferStatus) -> &'static str {
    match status {
        ProfileTransferStatus::Pending => "pending",
        ProfileTransferStatus::Accepted => "accepted",
        ProfileTransferStatus::Rejected => "rejected",
        ProfileTransferStatus::Cancelled => "cancelled",
    }
}
