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

#[cfg(test)]
mod tests {
    use super::*;

    fn request_id() -> ProfileTransferRequestId {
        ProfileTransferRequestId::new(1)
    }

    fn profile_id() -> ProfileId {
        ProfileId::new(10)
    }

    fn from_account_id() -> AccountId {
        AccountId::new(100)
    }

    fn to_org_account_id() -> AccountId {
        AccountId::new(200)
    }

    fn nanoid() -> Nanoid<ProfileTransferRequest> {
        Nanoid::new("ptr-test")
    }

    fn apply_requested() -> ProfileTransferRequest {
        let id = request_id();
        let command = ProfileTransferRequest::request(
            id.clone(),
            profile_id(),
            from_account_id(),
            to_org_account_id(),
            nanoid(),
        );
        let envelope = EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::default(),
        );
        let mut request = None;
        ProfileTransferRequest::apply(&mut request, envelope).unwrap();
        request.unwrap()
    }

    #[test]
    fn request_creates_pending_transfer() {
        crate::ensure_generator_initialized();
        let request = apply_requested();
        assert_eq!(request.id(), &request_id());
        assert_eq!(request.profile_id(), &profile_id());
        assert_eq!(request.from_account_id(), &from_account_id());
        assert_eq!(request.to_org_account_id(), &to_org_account_id());
        assert_eq!(request.status(), &ProfileTransferStatus::Pending);
        assert_eq!(request.nanoid(), &nanoid());
    }

    #[test]
    fn accept_transitions_pending_to_accepted() {
        crate::ensure_generator_initialized();
        let request = apply_requested();
        let command =
            ProfileTransferRequest::accept(request.id().clone(), request.version().clone());
        let envelope = EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(request.id().as_ref() + 1),
        );
        let mut request = Some(request);
        ProfileTransferRequest::apply(&mut request, envelope).unwrap();
        let request = request.unwrap();
        assert_eq!(request.status(), &ProfileTransferStatus::Accepted);
    }

    #[test]
    fn reject_transitions_pending_to_rejected() {
        crate::ensure_generator_initialized();
        let request = apply_requested();
        let command =
            ProfileTransferRequest::reject(request.id().clone(), request.version().clone());
        let envelope = EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(request.id().as_ref() + 1),
        );
        let mut request = Some(request);
        ProfileTransferRequest::apply(&mut request, envelope).unwrap();
        let request = request.unwrap();
        assert_eq!(request.status(), &ProfileTransferStatus::Rejected);
    }

    #[test]
    fn cancel_transitions_pending_to_cancelled() {
        crate::ensure_generator_initialized();
        let request = apply_requested();
        let command =
            ProfileTransferRequest::cancel(request.id().clone(), request.version().clone());
        let envelope = EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(request.id().as_ref() + 1),
        );
        let mut request = Some(request);
        ProfileTransferRequest::apply(&mut request, envelope).unwrap();
        let request = request.unwrap();
        assert_eq!(request.status(), &ProfileTransferStatus::Cancelled);
    }

    #[test]
    fn accept_on_already_decided_request_fails() {
        crate::ensure_generator_initialized();
        let request = apply_requested();
        let command =
            ProfileTransferRequest::accept(request.id().clone(), request.version().clone());
        let envelope = EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(request.id().as_ref() + 1),
        );
        let mut request = Some(request);
        ProfileTransferRequest::apply(&mut request, envelope).unwrap();

        let next_command = ProfileTransferRequest::reject(
            request.as_ref().unwrap().id().clone(),
            request.as_ref().unwrap().version().clone(),
        );
        let next_envelope = EventEnvelope::new(
            next_command.id().clone(),
            next_command.event().clone(),
            EventVersion::new(request.as_ref().unwrap().id().as_ref() + 2),
        );
        let err = ProfileTransferRequest::apply(&mut request, next_envelope).unwrap_err();
        assert_eq!(err.current_context(), &KernelError::Rejected);
    }

    #[test]
    fn requested_event_on_existing_request_fails() {
        crate::ensure_generator_initialized();
        let request = apply_requested();
        let command = ProfileTransferRequest::request(
            request.id().clone(),
            request.profile_id().clone(),
            request.from_account_id().clone(),
            request.to_org_account_id().clone(),
            request.nanoid().clone(),
        );
        let envelope = EventEnvelope::new(
            command.id().clone(),
            command.event().clone(),
            EventVersion::new(request.id().as_ref() + 1),
        );
        let mut request = Some(request);
        let err = ProfileTransferRequest::apply(&mut request, envelope).unwrap_err();
        assert_eq!(err.current_context(), &KernelError::Internal);
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
