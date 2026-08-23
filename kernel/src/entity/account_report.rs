mod id;

pub use self::id::*;

use super::{
    AccountId, CommandEnvelope, EventEnvelope, EventId, EventVersion, ExpectedVersion, Nanoid,
};
use crate::event::EventApplier;
use crate::KernelError;
use destructure::Destructure;
use error_stack::Report;
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Nameln, Newln, References};

#[derive(
    Debug, Clone, Hash, Eq, PartialEq, References, Newln, Destructure, Serialize, Deserialize,
)]
pub struct AccountReport {
    id: AccountReportId,
    target: AccountId,
    reported_by: AccountId,
    category: ReportCategory,
    comment: Option<ReportComment>,
    status: ReportStatus,
    version: EventVersion<AccountReport>,
    nanoid: Nanoid<AccountReport>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportCategory {
    Spam,
    Harassment,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReportStatus {
    Open,
    Closed {
        resolution: ReportResolution,
        close_reason: CloseReason,
    },
}

impl ReportStatus {
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportResolution {
    Resolved,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct ReportComment(String);

impl ReportComment {
    pub const MAX_LENGTH: usize = 500;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.trim().is_empty() {
            return Err(Report::new(KernelError::Validation)
                .attach_printable("Report comment cannot be empty".to_string()));
        }
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Report comment must not exceed {} characters",
                    Self::MAX_LENGTH
                )),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct CloseReason(String);

impl CloseReason {
    pub const MAX_LENGTH: usize = 500;

    pub fn validate(&self) -> error_stack::Result<(), KernelError> {
        if self.0.trim().is_empty() {
            return Err(Report::new(KernelError::Validation)
                .attach_printable("Close reason cannot be empty".to_string()));
        }
        if self.0.chars().count() > Self::MAX_LENGTH {
            return Err(
                Report::new(KernelError::Validation).attach_printable(format!(
                    "Close reason must not exceed {} characters",
                    Self::MAX_LENGTH
                )),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Nameln, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[vodca(prefix = "account_report", snake_case)]
pub enum AccountReportEvent {
    Created {
        target: AccountId,
        reported_by: AccountId,
        category: ReportCategory,
        comment: Option<ReportComment>,
        nanoid: Nanoid<AccountReport>,
    },
    Closed {
        resolution: ReportResolution,
        close_reason: CloseReason,
    },
}

impl AccountReport {
    pub fn create(
        id: AccountReportId,
        target: AccountId,
        reported_by: AccountId,
        category: ReportCategory,
        comment: Option<ReportComment>,
        nanoid: Nanoid<AccountReport>,
    ) -> CommandEnvelope<AccountReportEvent, AccountReport> {
        let event = AccountReportEvent::Created {
            target,
            reported_by,
            category,
            comment,
            nanoid,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(ExpectedVersion::Nothing),
        )
    }

    pub fn close(
        id: AccountReportId,
        resolution: ReportResolution,
        close_reason: CloseReason,
        current_version: EventVersion<AccountReport>,
    ) -> CommandEnvelope<AccountReportEvent, AccountReport> {
        let event = AccountReportEvent::Closed {
            resolution,
            close_reason,
        };
        CommandEnvelope::new(
            EventId::from(id),
            event.name(),
            event,
            Some(ExpectedVersion::At(current_version)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reconstitute(
        id: AccountReportId,
        target: AccountId,
        reported_by: AccountId,
        category: ReportCategory,
        comment: Option<ReportComment>,
        status: ReportStatus,
        version: EventVersion<AccountReport>,
        nanoid: Nanoid<AccountReport>,
    ) -> Self {
        Self {
            id,
            target,
            reported_by,
            category,
            comment,
            status,
            version,
            nanoid,
        }
    }
}

impl EventApplier for AccountReport {
    type Event = AccountReportEvent;
    const ENTITY_NAME: &'static str = "AccountReport";

    fn apply(
        entity: &mut Option<Self>,
        event: EventEnvelope<Self::Event, Self>,
    ) -> error_stack::Result<(), KernelError> {
        match event.event {
            AccountReportEvent::Created {
                target,
                reported_by,
                category,
                comment,
                nanoid,
            } => {
                if let Some(entity) = entity {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::already_exists(entity)));
                }
                *entity = Some(AccountReport {
                    id: AccountReportId::new(event.id),
                    target,
                    reported_by,
                    category,
                    comment,
                    status: ReportStatus::Open,
                    version: event.version,
                    nanoid,
                });
            }
            AccountReportEvent::Closed {
                resolution,
                close_reason,
            } => {
                if let Some(report) = entity {
                    if report.status.is_closed() {
                        return Err(Report::new(KernelError::Rejected)
                            .attach_printable("Account report is already closed"));
                    }
                    report.status = ReportStatus::Closed {
                        resolution,
                        close_reason,
                    };
                    report.version = event.version;
                } else {
                    return Err(Report::new(KernelError::Internal)
                        .attach_printable(Self::not_exists(event.id.as_ref())));
                }
            }
        }
        Ok(())
    }
}
