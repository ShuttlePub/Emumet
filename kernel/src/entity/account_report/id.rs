use crate::entity::{AccountReport, AccountReportEvent, EventId};
use serde::{Deserialize, Serialize};
use vodca::{AsRefln, Fromln, Newln};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Fromln, AsRefln, Newln, Serialize, Deserialize)]
pub struct AccountReportId(i64);

impl From<AccountReportId> for EventId<AccountReportEvent, AccountReport> {
    fn from(account_report_id: AccountReportId) -> Self {
        EventId::new(account_report_id.0)
    }
}
