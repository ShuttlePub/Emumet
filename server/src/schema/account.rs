use application::transfer::account::{CreateAccountDto, ModerationDto, UpdateAccountDto};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

#[derive(Debug, Deserialize, ToSchema)]
pub struct GetAllAccountQuery {
    pub ids: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub direction: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateAccountRequest {
    pub name: String,
    pub is_bot: bool,
}

impl CreateAccountRequest {
    pub fn into_dto(self) -> CreateAccountDto {
        CreateAccountDto {
            name: self.name,
            is_bot: self.is_bot,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct UpdateAccountRequest {
    pub is_bot: bool,
}

impl UpdateAccountRequest {
    pub fn into_dto(self, account_nanoid: String) -> UpdateAccountDto {
        UpdateAccountDto {
            account_nanoid,
            is_bot: self.is_bot,
        }
    }
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct SuspendAccountRequest {
    pub reason: String,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct BanAccountRequest {
    pub reason: String,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountResponse {
    pub id: String,
    pub name: String,
    pub public_key: String,
    pub is_bot: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moderation: Option<ModerationResponse>,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModerationResponse {
    Suspended {
        reason: String,
        #[serde(with = "time::serde::rfc3339")]
        suspended_at: OffsetDateTime,
        #[serde(
            skip_serializing_if = "Option::is_none",
            with = "time::serde::rfc3339::option"
        )]
        expires_at: Option<OffsetDateTime>,
    },
    Banned {
        reason: String,
        #[serde(with = "time::serde::rfc3339")]
        banned_at: OffsetDateTime,
    },
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AccountsResponse {
    pub first: Option<String>,
    pub last: Option<String>,
    pub items: Vec<AccountResponse>,
}

pub fn to_moderation_response(dto: Option<&ModerationDto>) -> Option<ModerationResponse> {
    dto.map(|m| match m {
        ModerationDto::Suspended {
            reason,
            suspended_at,
            expires_at,
        } => ModerationResponse::Suspended {
            reason: reason.clone(),
            suspended_at: *suspended_at,
            expires_at: *expires_at,
        },
        ModerationDto::Banned { reason, banned_at } => ModerationResponse::Banned {
            reason: reason.clone(),
            banned_at: *banned_at,
        },
    })
}

pub fn account_dto_to_response(
    account: application::transfer::account::AccountDto,
) -> AccountResponse {
    AccountResponse {
        id: account.nanoid,
        name: account.name,
        public_key: account.public_key,
        is_bot: account.is_bot,
        created_at: account.created_at,
        moderation: to_moderation_response(account.moderation.as_ref()),
    }
}
