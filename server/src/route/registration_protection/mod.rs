mod admin;
mod public;

pub(crate) use admin::{
    __path_list_registration_protections, __path_verify_registration_protection,
    list_registration_protections, verify_registration_protection,
};
pub(crate) use public::{
    __path_confirm_email_challenge, __path_create_email_challenge, confirm_email_challenge,
    create_email_challenge,
};

use crate::handler::AppModule;
use axum::routing::{get, post};
use axum::Router;

pub trait AdminRegistrationProtectionRouter {
    fn route_admin_registration_protection(self) -> Self;
}

impl AdminRegistrationProtectionRouter for Router<AppModule> {
    fn route_admin_registration_protection(self) -> Self {
        self.route(
            "/registration-gate/protections",
            get(list_registration_protections),
        )
        .route(
            "/registration-gate/protections/{protection}/verify",
            post(verify_registration_protection),
        )
    }
}

/// 未認証の登録前 email challenge。`/api/v1` の認証付き nest とは別に、完全なパスで登録する。
pub trait RegistrationEmailRouter {
    fn route_registration_email(self) -> Self;
}

impl RegistrationEmailRouter for Router<AppModule> {
    fn route_registration_email(self) -> Self {
        self.route(
            "/api/v1/registration/email-challenges",
            post(create_email_challenge),
        )
        .route(
            "/api/v1/registration/email-challenges/{challenge_id}/confirm",
            post(confirm_email_challenge),
        )
    }
}

#[cfg(test)]
mod tests;
