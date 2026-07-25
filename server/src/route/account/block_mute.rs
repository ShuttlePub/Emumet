use crate::auth::{resolve_auth_account_id, AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::handler::AppModule;
use crate::schema::account::{
    BlockAccountRequest, MuteAccountRequest, RelationListResponse, RelationResponse,
};
use application::service::block::{BlockAccountUseCase, GetBlocksUseCase, UnblockAccountUseCase};
use application::service::mute::{GetMutesUseCase, MuteAccountUseCase, UnmuteAccountUseCase};
use application::transfer::block_mute::{BlockAccountDto, MuteAccountDto, RelationDto};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

fn relation_dto_to_response(dto: RelationDto) -> RelationResponse {
    RelationResponse {
        id: dto.id,
        target_type: dto.target_type,
        target: dto.target,
    }
}

fn validate_relation_request(account_id: &str, target: &str) -> Result<(), ErrorStatus> {
    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }
    if target.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Target cannot be empty".to_string(),
        )));
    }
    Ok(())
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/block",
    description = "Block a local or remote account. Also removes any follow relationships between the two accounts in both directions.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    request_body = BlockAccountRequest,
    responses(
        (status = 200, description = "Account blocked", body = RelationResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Account not found"),
        (status = 422, description = "Cannot block (self-block or already blocked)"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn block_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(request): Json<BlockAccountRequest>,
) -> Result<Json<RelationResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);
    validate_relation_request(&account_id, &request.target)?;

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let result = module
        .block_account(
            auth_account_id,
            BlockAccountDto {
                account_nanoid: account_id,
                target: request.target,
            },
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(relation_dto_to_response(result)))
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/unblock",
    description = "Remove a block on a local or remote account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    request_body = BlockAccountRequest,
    responses(
        (status = 204, description = "Block removed"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Account or block relationship not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn unblock_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(request): Json<BlockAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);
    validate_relation_request(&account_id, &request.target)?;

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .unblock_account(
            auth_account_id,
            BlockAccountDto {
                account_nanoid: account_id,
                target: request.target,
            },
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}/blocks",
    description = "List accounts blocked by the given account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    responses(
        (status = 200, description = "Blocked accounts", body = RelationListResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn get_blocks(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Json<RelationListResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let blocks = module
        .get_blocks(auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(RelationListResponse {
        items: blocks.into_iter().map(relation_dto_to_response).collect(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/mute",
    description = "Mute a local or remote account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    request_body = MuteAccountRequest,
    responses(
        (status = 200, description = "Account muted", body = RelationResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Account not found"),
        (status = 422, description = "Cannot mute (self-mute or already muted)"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn mute_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(request): Json<MuteAccountRequest>,
) -> Result<Json<RelationResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);
    validate_relation_request(&account_id, &request.target)?;

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let result = module
        .mute_account(
            auth_account_id,
            MuteAccountDto {
                account_nanoid: account_id,
                target: request.target,
            },
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(relation_dto_to_response(result)))
}

#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/unmute",
    description = "Remove a mute on a local or remote account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    request_body = MuteAccountRequest,
    responses(
        (status = 204, description = "Mute removed"),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Account or mute relationship not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn unmute_account(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
    Json(request): Json<MuteAccountRequest>,
) -> Result<StatusCode, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);
    validate_relation_request(&account_id, &request.target)?;

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    module
        .unmute_account(
            auth_account_id,
            MuteAccountDto {
                account_nanoid: account_id,
                target: request.target,
            },
        )
        .await
        .map_err(ErrorStatus::from)?;

    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}/mutes",
    description = "List accounts muted by the given account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    responses(
        (status = 200, description = "Muted accounts", body = RelationListResponse),
        (status = 400, description = "Invalid request"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn get_mutes(
    Extension(claims): Extension<AuthClaims>,
    State(module): State<AppModule>,
    Path(account_id): Path<String>,
) -> Result<Json<RelationListResponse>, ErrorStatus> {
    let auth_info = OidcAuthInfo::from(claims);

    if account_id.trim().is_empty() {
        return Err(ErrorStatus::from((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )));
    }

    let auth_account_id = resolve_auth_account_id(&module, auth_info)
        .await
        .map_err(ErrorStatus::from)?;

    let mutes = module
        .get_mutes(auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?;

    Ok(Json(RelationListResponse {
        items: mutes.into_iter().map(relation_dto_to_response).collect(),
    }))
}

#[cfg(test)]
mod test {
    use crate::handler::AppModule;
    use application::service::block::remove_follows_between;
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
    use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
    use kernel::interfaces::repository::{DependOnFollowRepository, FollowRepository};
    use kernel::prelude::entity::{AccountId, FollowTargetId};
    use kernel::test_utils::{unique_account_name, AccountBuilder, FollowBuilder};

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn remove_follows_between_removes_follows_in_both_directions() {
        kernel::ensure_generator_initialized();
        let module = AppModule::new_for_oauth2_test(
            "http://localhost:65535".into(),
            "http://localhost:65535".into(),
        )
        .await
        .unwrap();
        let mut executor = module.database_connection().get_executor().await.unwrap();

        let a_id = AccountId::default();
        let account_a = AccountBuilder::new()
            .id(a_id.clone())
            .name(unique_account_name())
            .build();
        module
            .account_read_model()
            .create(&mut executor, &account_a)
            .await
            .unwrap();
        let b_id = AccountId::default();
        let account_b = AccountBuilder::new()
            .id(b_id.clone())
            .name(unique_account_name())
            .build();
        module
            .account_read_model()
            .create(&mut executor, &account_b)
            .await
            .unwrap();

        let follow_a_to_b = FollowBuilder::new()
            .source_local(a_id.clone())
            .destination_local(b_id.clone())
            .build();
        module
            .follow_repository()
            .create(&mut executor, &follow_a_to_b)
            .await
            .unwrap();
        let follow_b_to_a = FollowBuilder::new()
            .source_local(b_id.clone())
            .destination_local(a_id.clone())
            .build();
        module
            .follow_repository()
            .create(&mut executor, &follow_b_to_a)
            .await
            .unwrap();

        let a = FollowTargetId::from(a_id.clone());
        let b = FollowTargetId::from(b_id.clone());
        remove_follows_between(module.follow_repository(), &mut executor, &a, &b)
            .await
            .unwrap();

        let followings_of_a = module
            .follow_repository()
            .find_followings(&mut executor, &a)
            .await
            .unwrap();
        assert!(followings_of_a.is_empty());
        let followings_of_b = module
            .follow_repository()
            .find_followings(&mut executor, &b)
            .await
            .unwrap();
        assert!(followings_of_b.is_empty());

        module
            .account_read_model()
            .deactivate(&mut executor, account_a.id())
            .await
            .unwrap();
        module
            .account_read_model()
            .deactivate(&mut executor, account_b.id())
            .await
            .unwrap();
    }
}
