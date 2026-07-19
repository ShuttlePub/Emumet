use application::transfer::account::{
    AccountDetailDto, AccountFieldDto, CreateAccountDto, ModerationDto, UpdateAccountDto,
};
use kernel::prelude::entity::FieldAction;
use serde::{Deserialize, Deserializer, Serialize};
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
    #[serde(default, deserialize_with = "deserialize_optional_nullable_bool")]
    #[schema(nullable)]
    pub is_bot: Option<Option<bool>>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    #[schema(nullable)]
    pub display_name: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    #[schema(nullable)]
    pub summary: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    #[schema(nullable)]
    pub icon_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_string")]
    #[schema(nullable)]
    pub banner_url: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_nullable_fields")]
    #[schema(nullable)]
    pub fields: Option<Option<Vec<AccountField>>>,
}

impl UpdateAccountRequest {
    pub fn into_dto(self, account_nanoid: String) -> Result<UpdateAccountDto, &'static str> {
        let fields = self
            .fields
            .map(|value| value.ok_or("fields must be an array when present"))
            .transpose()?
            .map(|fields| fields.into_iter().map(AccountFieldDto::from).collect());
        Ok(UpdateAccountDto {
            account_nanoid,
            is_bot: into_field_action(self.is_bot),
            display_name: into_field_action(self.display_name),
            summary: into_field_action(self.summary),
            icon_url: into_field_action(self.icon_url),
            banner_url: into_field_action(self.banner_url),
            fields,
        })
    }
}

fn into_field_action<T>(value: Option<Option<T>>) -> FieldAction<T> {
    match value {
        None => FieldAction::Unchanged,
        Some(None) => FieldAction::Clear,
        Some(Some(value)) => FieldAction::Set(value),
    }
}

fn deserialize_optional_nullable_string<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

fn deserialize_optional_nullable_bool<'de, D>(
    deserializer: D,
) -> Result<Option<Option<bool>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<bool>::deserialize(deserializer).map(Some)
}

fn deserialize_optional_nullable_fields<'de, D>(
    deserializer: D,
) -> Result<Option<Option<Vec<AccountField>>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Vec<AccountField>>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema, Eq, PartialEq)]
pub struct AccountField {
    pub label: String,
    pub content: String,
}

impl From<AccountField> for AccountFieldDto {
    fn from(field: AccountField) -> Self {
        Self {
            label: field.label,
            content: field.content,
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
    pub display_name: Option<String>,
    pub summary: Option<String>,
    pub icon_url: Option<String>,
    pub banner_url: Option<String>,
    pub is_bot: bool,
    pub fields: Vec<AccountField>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
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

#[derive(Debug, Deserialize, ToSchema)]
pub struct FollowAccountRequest {
    /// Remote actor URL (e.g. https://remote.example/users/bob) or acct:user@domain
    pub target: String,
}

#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct FollowAccountResponse {
    pub follow_id: String,
    pub remote_actor_url: String,
    pub activity_id: String,
    pub approved: bool,
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

pub fn account_dto_to_response(account: AccountDetailDto) -> AccountResponse {
    AccountResponse {
        id: account.nanoid,
        name: account.name,
        display_name: account.display_name,
        summary: account.summary,
        icon_url: account.icon_url,
        banner_url: account.banner_url,
        is_bot: account.is_bot,
        fields: account
            .fields
            .into_iter()
            .map(|field| AccountField {
                label: field.label,
                content: field.content,
            })
            .collect(),
        created_at: account.created_at,
        moderation: to_moderation_response(account.moderation.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel::prelude::entity::FieldAction;

    #[test]
    fn patch_request_distinguishes_absent_null_and_value() {
        let request: UpdateAccountRequest =
            serde_json::from_str(r#"{"display_name":"Display","summary":null,"fields":[]}"#)
                .unwrap();

        let dto = request.into_dto("account-id".to_string()).unwrap();

        assert!(matches!(dto.is_bot, FieldAction::Unchanged));
        assert!(matches!(dto.display_name, FieldAction::Set(value) if value == "Display"));
        assert!(matches!(dto.summary, FieldAction::Clear));
        assert!(matches!(dto.icon_url, FieldAction::Unchanged));
        assert_eq!(dto.fields, Some(Vec::new()));
    }

    #[test]
    fn patch_request_deserializes_explicit_null_as_clear() {
        let request: UpdateAccountRequest = serde_json::from_str(r#"{"summary":null}"#).unwrap();

        let dto = request.into_dto("account-id".to_string()).unwrap();

        assert!(matches!(dto.summary, FieldAction::Clear));
    }
}
