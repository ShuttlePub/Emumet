use crate::database::{Connection, DatabaseConnection, DependOnDatabaseConnection};
use crate::KernelError;
use std::future::Future;
use time::OffsetDateTime;

/// An internal, non-personal negative canary namespace. Never a registration bypass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnforcementProbeId(String);

impl EnforcementProbeId {
    pub fn parse(value: &str) -> Option<Self> {
        (value.len() == 32
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then(|| Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn email(&self) -> String {
        format!("emumet-probe-{}@registration-canary.invalid", self.0)
    }

    pub fn is_reserved_email(email: &str) -> bool {
        email
            .to_ascii_lowercase()
            .ends_with("@registration-canary.invalid")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementProbeMode {
    Api,
    Browser,
}

impl EnforcementProbeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Browser => "browser",
        }
    }
}

/// No Debug: the temporary browser CSRF cookie/token must not enter logs.
pub struct EnforcementProbeFlow {
    pub target_key: String,
    pub flow_id: String,
    pub mode: EnforcementProbeMode,
    pub csrf_cookie: Option<String>,
    pub csrf_token: Option<String>,
}

#[derive(Debug, Clone)]
pub struct EnforcementCanary {
    pub id: EnforcementProbeId,
    pub target_key: String,
    pub flow_id: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone)]
pub struct EnforcementProbeEvidence {
    pub generation: String,
    pub verified: bool,
    pub checked_at: OffsetDateTime,
    pub valid_until: OffsetDateTime,
}

impl EnforcementProbeEvidence {
    pub fn is_current(&self, generation: &str, now: OffsetDateTime) -> bool {
        self.verified
            && self.generation == generation
            && self.checked_at <= now
            && now < self.valid_until
    }
}

pub trait RegistrationEnforcementProbeRepository: Sync + Send + 'static {
    type Connection: Connection;

    /// The caller must retain this transaction until the complete cycle ends.
    fn begin_cycle(
        &self,
        executor: &mut Self::Connection,
        at: OffsetDateTime,
        interval: time::Duration,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn evidence(
        &self,
        executor: &mut Self::Connection,
    ) -> impl Future<Output = error_stack::Result<Option<EnforcementProbeEvidence>, KernelError>> + Send;

    fn save_evidence(
        &self,
        executor: &mut Self::Connection,
        evidence: &EnforcementProbeEvidence,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn prepare(
        &self,
        executor: &mut Self::Connection,
        canary: &EnforcementCanary,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn receive(
        &self,
        executor: &mut Self::Connection,
        id: &EnforcementProbeId,
        flow_id: &str,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn was_received(
        &self,
        executor: &mut Self::Connection,
        id: &EnforcementProbeId,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn settle(
        &self,
        executor: &mut Self::Connection,
        id: &EnforcementProbeId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    /// Uncertain submissions remain monitored beyond Kratos' 93-second retry window.
    fn pending_cleanup(
        &self,
        executor: &mut Self::Connection,
        now: OffsetDateTime,
    ) -> impl Future<Output = error_stack::Result<Vec<EnforcementCanary>, KernelError>> + Send;
}

pub trait DependOnRegistrationEnforcementProbeRepository:
    Sync + Send + DependOnDatabaseConnection
{
    type RegistrationEnforcementProbeRepository: RegistrationEnforcementProbeRepository<
        Connection = <Self::DatabaseConnection as DatabaseConnection>::Connection,
    >;

    fn registration_enforcement_probe_repository(
        &self,
    ) -> &Self::RegistrationEnforcementProbeRepository;
}

/// Public Kratos registration plus restricted admin cleanup of this canary namespace only.
pub trait RegistrationEnforcementProbeClient: Sync + Send + 'static {
    fn generation(&self) -> &str;
    fn target_keys(&self) -> Vec<String>;

    fn initialize(
        &self,
        target_key: &str,
        mode: EnforcementProbeMode,
    ) -> impl Future<Output = error_stack::Result<EnforcementProbeFlow, KernelError>> + Send;

    /// True only for the structured, parsed guard denial in the public registration response.
    fn submit(
        &self,
        flow: &EnforcementProbeFlow,
        probe_id: &EnforcementProbeId,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn created_identities(
        &self,
        target_key: &str,
        probe_id: &EnforcementProbeId,
    ) -> impl Future<Output = error_stack::Result<Vec<String>, KernelError>> + Send;

    /// Implementations must re-check the exact generated canary email before deletion.
    fn delete_identity(
        &self,
        target_key: &str,
        probe_id: &EnforcementProbeId,
        identity_id: &str,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnRegistrationEnforcementProbeClient: Sync + Send {
    type RegistrationEnforcementProbeClient: RegistrationEnforcementProbeClient;
    fn registration_enforcement_probe_client(&self) -> &Self::RegistrationEnforcementProbeClient;
}
