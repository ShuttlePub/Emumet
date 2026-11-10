//! Internal final-submit enforcement. No public observation/override input and no verdict cache.
use super::registration_protection::{
    BotTokenVerdict, RegistrationEmailChallengeUseCase, RegistrationProtectionDeps,
    RegistrationProtectionReadinessUseCase,
};
use kernel::interfaces::repository::ProofVerification;
use kernel::prelude::entity::{Protection, RegistrationDecision};
use kernel::KernelError;
use std::future::Future;

/// Deliberately not Debug: tokens, proofs and email must not reach logs.
pub struct RegistrationAttempt<'a> {
    pub flow_id: &'a str,
    pub email: &'a str,
    pub email_proof: Option<&'a str>,
    pub bot_token: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationVerdict {
    Accept,
    Reject,
    TemporarilyUnavailable,
}

pub struct CheckedRegistration {
    pub verdict: RegistrationVerdict,
    verified: Vec<Protection>,
}

impl CheckedRegistration {
    pub fn covers(&self, decision: &RegistrationDecision) -> bool {
        self.verdict == RegistrationVerdict::Accept
            && matches!(decision, RegistrationDecision::Accept { enforced, .. }
                if enforced.iter().all(|protection| self.verified.contains(protection)))
    }
}

impl From<RegistrationVerdict> for CheckedRegistration {
    fn from(verdict: RegistrationVerdict) -> Self {
        Self {
            verdict,
            verified: Vec::new(),
        }
    }
}

pub trait EnforceRegistrationUseCase: RegistrationProtectionDeps {
    fn enforce_registration(
        &self,
        attempt: RegistrationAttempt<'_>,
    ) -> impl Future<Output = error_stack::Result<RegistrationVerdict, KernelError>> + Send {
        async move {
            self.enforce_registration_checked(attempt)
                .await
                .map(|checked| checked.verdict)
        }
    }

    fn enforce_registration_checked(
        &self,
        attempt: RegistrationAttempt<'_>,
    ) -> impl Future<Output = error_stack::Result<CheckedRegistration, KernelError>> + Send {
        async move {
            let gate = self.sync_registration_observation().await?;
            let enforced = match gate.decision {
                RegistrationDecision::Accept { enforced, .. } => enforced,
                RegistrationDecision::TemporaryReject => {
                    return Ok(RegistrationVerdict::TemporarilyUnavailable.into())
                }
                _ => return Ok(RegistrationVerdict::Reject.into()),
            };
            let mut verified = Vec::new();
            for protection in enforced {
                match protection {
                    Protection::Email => {
                        let Some(proof) = attempt.email_proof else {
                            return Ok(RegistrationVerdict::Reject.into());
                        };
                        if !matches!(
                            self.verify_registration_email_proof(
                                attempt.flow_id,
                                attempt.email,
                                proof
                            )
                            .await?,
                            ProofVerification::Verified { .. }
                        ) {
                            return Ok(RegistrationVerdict::Reject.into());
                        }
                        verified.push(protection);
                    }
                    Protection::Bot => {
                        let Some(token) =
                            attempt.bot_token.filter(|value| !value.trim().is_empty())
                        else {
                            return Ok(RegistrationVerdict::Reject.into());
                        };
                        match self.verify_registration_bot_token(token, None).await? {
                            BotTokenVerdict::Verified => verified.push(protection),
                            BotTokenVerdict::Rejected(_) => {
                                return Ok(RegistrationVerdict::Reject.into())
                            }
                            // Provider failure updates readiness. The healthy protection may still
                            // satisfy the final decision, but a bad user token never causes degradation.
                            BotTokenVerdict::ProviderFailure(_, _) => {}
                        }
                    }
                }
            }
            // Do not keep the pre-verification verdict. A closure or newly effective protection
            // during provider I/O must be honored. No cross-database atomicity is claimed after this.
            let gate = self.sync_registration_observation().await?;
            let verdict = match gate.decision {
                RegistrationDecision::Accept { enforced, .. }
                    if enforced.iter().all(|p| verified.contains(p)) =>
                {
                    RegistrationVerdict::Accept
                }
                RegistrationDecision::AdminClosed | RegistrationDecision::Stopped => {
                    RegistrationVerdict::Reject
                }
                _ => RegistrationVerdict::TemporarilyUnavailable,
            };
            Ok(CheckedRegistration { verdict, verified })
        }
    }
}
impl<T: RegistrationProtectionDeps> EnforceRegistrationUseCase for T {}
