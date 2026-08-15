use crate::api::AccountApi;
use crate::auth::{AuthClaims, OidcAuthInfo};
use crate::error::ErrorStatus;
use crate::schema::account::{RelationListResponse, RelationResponse};
use application::dto::activitypub::FollowRelationDto;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};

fn relation_response(dto: FollowRelationDto) -> RelationResponse {
    RelationResponse {
        id: dto.id,
        target_type: dto.target_type,
        target: dto.target,
    }
}

fn validate_account_id(account_id: &str) -> Result<(), ErrorStatus> {
    if account_id.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "Account ID cannot be empty".to_string(),
        )
            .into());
    }
    Ok(())
}

#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}/followers",
    description = "List approved followers of the given account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    responses(
        (status = 200, description = "Approved followers", body = RelationListResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Insufficient permission"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn get_followers(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
    Path(account_id): Path<String>,
) -> Result<Json<RelationListResponse>, ErrorStatus> {
    validate_account_id(&account_id)?;
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let items = api
        .get_followers(auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?
        .into_iter()
        .map(relation_response)
        .collect();
    Ok(Json(RelationListResponse { items }))
}

#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}/following",
    description = "List approved accounts followed by the given account.",
    params(("account_id" = String, Path, description = "Local account nanoid")),
    responses(
        (status = 200, description = "Approved following accounts", body = RelationListResponse),
        (status = 400, description = "Invalid request"),
        (status = 403, description = "Insufficient permission"),
        (status = 404, description = "Account not found"),
    ),
    security(("bearer_auth" = [])),
    tag = "Account",
)]
pub(crate) async fn get_following(
    Extension(claims): Extension<AuthClaims>,
    State(api): State<AccountApi>,
    Path(account_id): Path<String>,
) -> Result<Json<RelationListResponse>, ErrorStatus> {
    validate_account_id(&account_id)?;
    let auth_account_id = api
        .resolve_auth_account_id(OidcAuthInfo::from(claims))
        .await
        .map_err(ErrorStatus::from)?;
    let items = api
        .get_following(auth_account_id, account_id)
        .await
        .map_err(ErrorStatus::from)?
        .into_iter()
        .map(relation_response)
        .collect();
    Ok(Json(RelationListResponse { items }))
}

#[cfg(test)]
mod tests {
    use crate::handler::AppModule;
    use application::service::activitypub::relations::list_approved_relations;
    use kernel::interfaces::database::{DatabaseConnection, DependOnDatabaseConnection};
    use kernel::interfaces::read_model::DependOnAccountQuery;
    use kernel::interfaces::read_model::{AccountReadModel, DependOnAccountReadModel};
    use kernel::interfaces::repository::{
        DependOnFollowRepository, DependOnRemoteAccountRepository, FollowRepository,
        RemoteAccountRepository,
    };
    use kernel::prelude::entity::{AccountId, FollowApprovedAt, FollowTargetId};
    use kernel::test_utils::{
        unique_account_name, AccountBuilder, FollowBuilder, RemoteAccountBuilder,
    };

    #[test_with::env(DATABASE_URL)]
    #[tokio::test]
    async fn following_list_contains_only_approved_relations() {
        kernel::ensure_generator_initialized();
        let module = AppModule::new_for_oauth2_test(
            "http://localhost:65535".into(),
            "http://localhost:65535".into(),
        )
        .await
        .unwrap();
        let mut executor = module.database_connection().connection().await.unwrap();
        let source_id = AccountId::default();
        let source = AccountBuilder::new()
            .id(source_id.clone())
            .name(unique_account_name())
            .build();
        module
            .account_read_model()
            .create(&mut executor, &source)
            .await
            .unwrap();
        let approved_remote = RemoteAccountBuilder::new().build();
        module
            .remote_account_repository()
            .create(&mut executor, &approved_remote)
            .await
            .unwrap();
        let pending_remote = RemoteAccountBuilder::new().build();
        module
            .remote_account_repository()
            .create(&mut executor, &pending_remote)
            .await
            .unwrap();
        let approved = FollowBuilder::new()
            .source_local(source_id.clone())
            .destination(FollowTargetId::from(approved_remote.id().clone()))
            .approved_at(Some(FollowApprovedAt::default()))
            .build();
        module
            .follow_repository()
            .create(&mut executor, &approved)
            .await
            .unwrap();
        let pending = FollowBuilder::new()
            .source_local(source_id.clone())
            .destination(FollowTargetId::from(pending_remote.id().clone()))
            .build();
        module
            .follow_repository()
            .create(&mut executor, &pending)
            .await
            .unwrap();

        let relations = list_approved_relations(
            module.account_query(),
            module.follow_repository(),
            module.remote_account_repository(),
            &mut executor,
            &FollowTargetId::from(source_id),
            true,
        )
        .await
        .unwrap();

        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].id, approved.id().as_ref().to_string());
        assert_eq!(relations[0].target_type, "remote");
        assert_eq!(relations[0].target, approved_remote.url().as_ref().as_str());
    }
}
