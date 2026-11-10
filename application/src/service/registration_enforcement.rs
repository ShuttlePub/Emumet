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

pub trait EnforceRegistrationUseCase: RegistrationProtectionDeps {
    fn enforce_registration(
        &self,
        attempt: RegistrationAttempt<'_>,
    ) -> impl Future<Output = error_stack::Result<RegistrationVerdict, KernelError>> + Send {
        async move {
            let gate = self.sync_registration_observation().await?;
            let enforced = match gate.decision {
                RegistrationDecision::Accept { enforced, .. } => enforced,
                RegistrationDecision::TemporaryReject => {
                    return Ok(RegistrationVerdict::TemporarilyUnavailable)
                }
                _ => return Ok(RegistrationVerdict::Reject),
            };
            let mut verified = Vec::new();
            for protection in enforced {
                match protection {
                    Protection::Email => {
                        let Some(proof) = attempt.email_proof else {
                            return Ok(RegistrationVerdict::Reject);
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
                            return Ok(RegistrationVerdict::Reject);
                        }
                        verified.push(protection);
                    }
                    Protection::Bot => {
                        let Some(token) =
                            attempt.bot_token.filter(|value| !value.trim().is_empty())
                        else {
                            return Ok(RegistrationVerdict::Reject);
                        };
                        match self.verify_registration_bot_token(token, None).await? {
                            BotTokenVerdict::Verified => verified.push(protection),
                            BotTokenVerdict::Rejected(_) => return Ok(RegistrationVerdict::Reject),
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
            Ok(match gate.decision {
                RegistrationDecision::Accept { enforced, .. }
                    if enforced.iter().all(|p| verified.contains(p)) =>
                {
                    RegistrationVerdict::Accept
                }
                RegistrationDecision::AdminClosed | RegistrationDecision::Stopped => {
                    RegistrationVerdict::Reject
                }
                _ => RegistrationVerdict::TemporarilyUnavailable,
            })
        }
    }
}
impl<T: RegistrationProtectionDeps> EnforceRegistrationUseCase for T {}
