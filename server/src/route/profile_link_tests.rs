//! Real PostgreSQL + production-router acceptance tests for L1.
use super::*;
use crate::route::profile_graph_test_support::{GraphWorld, LocalProfile, User};
use application::dto::organization::CreateOrganizationDto;
use application::projection::ProjectAccountBatch;
use application::service::home_link::HomeLinkUseCase;
use application::service::organization::CreateOrganizationUseCase;
use application::service::profile_transfer::{
    AcceptProfileTransferRequestUseCase, RequestProfileTransferUseCase,
};
use axum::body::Body;
use axum::http::Request;
use driver::home_origin::HomeOriginAllowlist;
use driver::webhook::HostResolver;
use http_body_util::BodyExt;
use kernel::interfaces::database::{
    DatabaseConnection, DependOnDatabaseConnection, Transaction, TransactionalDatabaseConnection,
};
use kernel::interfaces::home_link::{DependOnHomeLinkStore, HomeLinkStore};
use kernel::interfaces::repository::{AggregateRepository, DependOnAccountRepository};
use kernel::prelude::entity::{Account, AccountEvent, AccountId, EventVersion};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;
struct PublicDns;
impl HostResolver for PublicDns {
    fn resolve<'a>(
        &'a self,
        _: &'a str,
        port: u16,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = std::io::Result<Vec<std::net::SocketAddr>>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move { Ok(vec![std::net::SocketAddr::from(([93, 184, 216, 34], port))]) })
    }
}
async fn world() -> GraphWorld {
    let mut w = GraphWorld::new().await;
    w.module = w.module.with_home_origin_policy(
        HomeOriginAllowlist::from_json(r#"["https://home.example","https://other.example"]"#)
            .unwrap()
            .with_resolver(Arc::new(PublicDns)),
    );
    w.allow_all_permissions().await;
    w
}
async fn request(
    w: &GraphWorld,
    u: Option<&User>,
    method: &str,
    path: &str,
    body: Option<Value>,
    org: Option<&str>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(u) = u {
        builder = builder.header("authorization", format!("Bearer {}", u.token));
    }
    if let Some(org) = org {
        builder = builder.header("X-Organization-Id", org);
    }
    let body = if let Some(v) = body {
        builder = builder.header("content-type", "application/json");
        Body::from(v.to_string())
    } else {
        Body::empty()
    };
    let response = w
        .root_router()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}
async fn register(w: &GraphWorld, u: &User, p: &LocalProfile) -> Value {
    let (s, l) = request(
        w,
        Some(u),
        "POST",
        &format!("/api/v1/profiles/{}/links", p.nanoid),
        Some(json!({"origin":"https://HOME.example:443/","trust_mode":3})),
        None,
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    l
}
async fn owner(w: &GraphWorld, p: &LocalProfile) -> AccountId {
    AccountId::new(
        sqlx::query_scalar::<_, i64>("SELECT account_id FROM profiles WHERE id=$1")
            .bind(p.id.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap(),
    )
}
async fn raw_event(w: &GraphWorld, id: &AccountId, event: AccountEvent) -> EventVersion<Account> {
    let version = kernel::generate_id();
    sqlx::query(
        "INSERT INTO account_events(version,id,event_name,data) VALUES ($1,$2,'l1_fixture',$3)",
    )
    .bind(version)
    .bind(id.as_ref())
    .bind(serde_json::to_value(event).unwrap())
    .execute(w.pool())
    .await
    .unwrap();
    EventVersion::new(version)
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_api_auth_origin_modes_and_idempotency_including_non_actor_profile() {
    let w = world().await;
    let u = w.new_user().await;
    let stranger = w.new_user().await;
    let p = w.create_profile(&u, false).await;
    let base = format!("/api/v1/profiles/{}/links", p.nanoid);
    assert_eq!(
        request(&w, None, "GET", &base, None, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    let l = register(&w, &u, &p).await;
    assert_eq!(l["origin"], "https://home.example");
    assert_eq!(l["generation"], 1);
    assert_eq!(
        request(
            &w,
            Some(&u),
            "POST",
            &base,
            Some(json!({"origin":"https://home.example","trust_mode":3})),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    for (payload, status) in [
        (
            json!({"origin":"https://home.example","trust_mode":3,"client_id":"not-supported"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"origin":"https://home.example"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"origin":"https://other.example","trust_mode":3}),
            StatusCode::CONFLICT,
        ),
        (
            json!({"origin":"https://home.example","trust_mode":1}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            json!({"origin":"http://home.example","trust_mode":3}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"origin":"https://not-allowed.example","trust_mode":3}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"origin":"https://127.0.0.1","trust_mode":3}),
            StatusCode::BAD_REQUEST,
        ),
    ] {
        assert_eq!(
            request(&w, Some(&u), "POST", &base, Some(payload), None)
                .await
                .0,
            status
        );
    }
    assert_eq!(
        request(&w, Some(&stranger), "GET", &base, None, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let delete = format!("{base}/{}", l["id"].as_str().unwrap());
    assert_eq!(
        request(&w, Some(&stranger), "DELETE", &delete, None, None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            request(&w, Some(&u), "DELETE", &delete, None, None).await.0,
            StatusCode::NO_CONTENT
        );
    }
    let (_, ls) = request(&w, Some(&u), "GET", &base, None, None).await;
    assert_eq!(ls["items"][0]["state"], "unlinked");
    assert_eq!(ls["items"][0]["generation"], 2);
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_transfer_preserves_identity_and_old_owner_cannot_list_register_or_unlink() {
    let w = world().await;
    let old = w.new_user().await;
    let new = w.new_user().await;
    let p = w.create_profile(&old, true).await;
    w.create_profile(&new, true).await;
    let l = register(&w, &old, &p).await;
    let org = w
        .module
        .create_organization(
            new.auth_account_id.clone(),
            CreateOrganizationDto {
                name: format!("l1org{}", kernel::generate_id()),
            },
        )
        .await
        .unwrap();
    // The requester must be a member of the target organization under existing transfer rules.
    let org_id = AccountId::new(
        sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE nanoid=$1")
            .bind(&org.account_id)
            .fetch_one(w.pool())
            .await
            .unwrap(),
    );
    let old_account = owner(&w, &p).await;
    sqlx::query("INSERT INTO organization_members(org_account_id,member_account_id,role,status,invited_by) SELECT $1,$2,'member','active',invited_by FROM organization_members WHERE org_account_id=$1 LIMIT 1")
        .bind(org_id.as_ref()).bind(old_account.as_ref()).execute(w.pool()).await.unwrap();
    let transfer = w
        .module
        .request_profile_transfer(
            &old.auth_account_id,
            p.nanoid.clone(),
            org.account_id.clone(),
        )
        .await
        .unwrap();
    w.module
        .accept_profile_transfer_request(&new.auth_account_id, transfer.nanoid)
        .await
        .unwrap();
    let base = format!("/api/v1/profiles/{}/links", p.nanoid);
    for (method, path, body) in [
        ("GET", base.clone(), None),
        (
            "POST",
            base.clone(),
            Some(json!({"origin":"https://home.example","trust_mode":3})),
        ),
        (
            "DELETE",
            format!("{base}/{}", l["id"].as_str().unwrap()),
            None,
        ),
    ] {
        assert_eq!(
            request(&w, Some(&old), method, &path, body, None).await.0,
            StatusCode::FORBIDDEN
        );
    }
    // Organization context remains required, even for the organization creator.
    assert_eq!(
        request(&w, Some(&new), "GET", &base, None, None).await.0,
        StatusCode::FORBIDDEN
    );
    let (s, ls) = request(&w, Some(&new), "GET", &base, None, Some(&org.account_id)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(ls["items"][0], l);
    w.module.project_batch().await.unwrap();
    let (_, again) = request(&w, Some(&new), "GET", &base, None, Some(&org.account_id)).await;
    assert_eq!(again, ls);
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_projector_same_batch_deactivate_reactivate_and_stale_replay_keep_new_link() {
    let w = world().await;
    let u = w.new_user().await;
    let p = w.create_profile(&u, true).await;
    let old = register(&w, &u, &p).await;
    let a = owner(&w, &p).await;
    // Bypass command hook deliberately: independently exercise projector defense.
    raw_event(&w, &a, AccountEvent::Deactivated).await;
    raw_event(&w, &a, AccountEvent::Reactivated).await;
    w.module.project_batch().await.unwrap();
    let ls = w
        .module
        .list_home_links(&u.auth_account_id, None, &p.nanoid)
        .await
        .unwrap();
    assert_eq!(ls[0].id, old["id"].as_str().unwrap());
    assert_eq!(ls[0].state, "unlinked");
    assert_eq!(ls[0].generation, 2);
    let fresh = register(&w, &u, &p).await;
    assert_ne!(fresh["id"], old["id"]);
    for _ in 0..2 {
        sqlx::query("DELETE FROM projection_checkpoints WHERE projector_name='account_projector'")
            .execute(w.pool())
            .await
            .unwrap();
        w.module.project_batch().await.unwrap();
    }
    let ls = w
        .module
        .list_home_links(&u.auth_account_id, None, &p.nanoid)
        .await
        .unwrap();
    assert_eq!(ls.iter().filter(|l| l.state == "active").count(), 1);
    assert!(ls
        .iter()
        .any(|l| l.id == fresh["id"].as_str().unwrap() && l.generation == 1));
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_projector_physical_delete_retains_ledger_and_reactivation_does_not_revive() {
    let w = world().await;
    let u = w.new_user().await;
    let p = w.create_profile(&u, true).await;
    register(&w, &u, &p).await;
    let a = owner(&w, &p).await;
    assert!(
        sqlx::query("DELETE FROM profiles WHERE id=$1")
            .bind(p.id.as_ref())
            .execute(w.pool())
            .await
            .is_err(),
        "uncoordinated deletion must be refused"
    );
    raw_event(&w, &a, AccountEvent::Deactivated).await;
    w.module.project_batch().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE id=$1")
            .bind(p.id.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap(),
        0
    );
    raw_event(&w, &a, AccountEvent::Reactivated).await;
    w.module.project_batch().await.unwrap();
    let links = w
        .module
        .list_home_links(&u.auth_account_id, None, &p.nanoid)
        .await
        .unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].state, "unlinked");
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_account_event_commit_and_projector_do_not_wait_for_http() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    let w = world().await;
    let u = w.new_user().await;
    let p = w.create_profile(&u, true).await;
    register(&w, &u, &p).await;
    let a = owner(&w, &p).await;
    let keto = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/admin/relation-tuples"))
        .respond_with(ResponseTemplate::new(201).set_delay(Duration::from_secs(2)))
        .mount(&keto)
        .await;
    let module = crate::handler::AppModule::new_for_test_database(
        driver::database::PostgresDatabase::from_pool(w.pool().clone()),
        "http://localhost:65535".into(),
        "http://localhost:65535".into(),
        keto.uri(),
        keto.uri(),
    )
    .await
    .unwrap();
    let db = module.database_connection().clone();
    let project = tokio::spawn(async move { module.project_batch().await });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !keto.received_requests().await.unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    // Projector is waiting for HTTP, but its Account transaction/gate is already gone.
    let started = std::time::Instant::now();
    let mut c = db.connection().await.unwrap();
    let loaded = db.account_repository().load(&mut c, &a).await.unwrap();
    db.account_repository()
        .save(
            &mut c,
            Account::deactivate(a.clone(), loaded.version().clone()),
        )
        .await
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(!project.is_finished());
    project.await.unwrap().unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT state FROM profile_links")
            .fetch_one(w.pool())
            .await
            .unwrap(),
        "unlinked"
    );
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_all_three_revokers_serialize_with_a_precommit_permit() {
    use application::service::account::DeactivateAccountUseCase;
    for kind in ["unlink", "command", "projector"] {
        let w = world().await;
        let u = w.new_user().await;
        let p = w.create_profile(&u, true).await;
        let l = register(&w, &u, &p).await;
        let a = owner(&w, &p).await;
        let db = w.module.database_connection();
        sqlx::query("CREATE TABLE l1_test_permits(link_id text)")
            .execute(w.pool())
            .await
            .unwrap();
        let mut permit = db.get_transaction().await.unwrap();
        w.module
            .home_link_store()
            .lock_accounts(permit.connection(), std::slice::from_ref(&a))
            .await
            .unwrap();
        w.module
            .home_link_store()
            .lock_profiles(permit.connection(), std::slice::from_ref(&p.id))
            .await
            .unwrap();
        sqlx::query("INSERT INTO l1_test_permits VALUES ($1)")
            .bind(l["id"].as_str().unwrap())
            .execute(&mut **permit.connection())
            .await
            .unwrap();
        // P1: the stand-in permit UPDATE is durable only after this held commit.
        if kind == "projector" {
            raw_event(&w, &a, AccountEvent::Deactivated).await;
        }
        let account_nanoid: String = sqlx::query_scalar("SELECT nanoid FROM accounts WHERE id=$1")
            .bind(a.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap();
        let module = w.module.clone();
        let auth = u.auth_account_id.clone();
        let profile = p.nanoid.clone();
        let id = l["id"].as_str().unwrap().to_owned();
        let revoke = tokio::spawn(async move {
            match kind {
                "unlink" => module.unlink_home_link(&auth, None, &profile, &id).await,
                "command" => module.deactivate_account(&auth, account_nanoid).await,
                _ => module.project_batch().await.map(|_| ()),
            }
        });
        wait_for_lock_waiter(&w).await;
        assert!(
            !revoke.is_finished(),
            "{kind} must wait for the permit transaction"
        );
        permit.commit().await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), revoke)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profile_links WHERE state='active'")
                .fetch_one(w.pool())
                .await
                .unwrap(),
            0,
            "{kind}"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM l1_test_permits")
                .fetch_one(w.pool())
                .await
                .unwrap(),
            1,
            "prior committed permit is the only allowed start"
        );
    }
}
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_projector_revokes_and_deletes_every_profile_of_an_organization() {
    use application::service::session_context::OrganizationContext;
    use kernel::interfaces::event::EventApplier;
    use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
    use kernel::interfaces::repository::DependOnProfileRepository;
    use kernel::prelude::entity::OrgRole;
    use kernel::prelude::entity::{Nanoid, Profile, ProfileId};
    let w = world().await;
    let u = w.new_user().await;
    w.create_profile(&u, true).await;
    let org = w
        .module
        .create_organization(
            u.auth_account_id.clone(),
            CreateOrganizationDto {
                name: format!("l1multi{}", kernel::generate_id()),
            },
        )
        .await
        .unwrap();
    let a = AccountId::new(
        sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE nanoid=$1")
            .bind(&org.account_id)
            .fetch_one(w.pool())
            .await
            .unwrap(),
    );
    let context = OrganizationContext {
        org_account_id: a.clone(),
        org_account_nanoid: org.account_id,
        role: OrgRole::Owner,
    };
    for _ in 0..3 {
        let mut c = w.module.database_connection().connection().await.unwrap();
        let event = w
            .module
            .profile_repository()
            .save(
                &mut c,
                Profile::create(
                    ProfileId::new(kernel::generate_id()),
                    a.clone(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                ),
            )
            .await
            .unwrap();
        let mut profile = None;
        Profile::apply(&mut profile, event).unwrap();
        let profile = profile.unwrap();
        w.module
            .profile_read_model()
            .create(&mut c, &profile)
            .await
            .unwrap();
        drop(c);
        w.module
            .register_home_link(
                &u.auth_account_id,
                Some(&context),
                profile.nanoid().as_ref(),
                "https://home.example",
                3,
            )
            .await
            .unwrap();
    }
    raw_event(&w, &a, AccountEvent::Deactivated).await;
    w.module.project_batch().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE account_id=$1")
            .bind(a.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM profile_links WHERE owner_account_id=$1 AND state='unlinked' AND generation=2").bind(a.as_ref()).fetch_one(w.pool()).await.unwrap(),3);
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_register_unlink_and_real_transfer_race_keeps_only_current_owner() {
    let w = world().await;
    let old = w.new_user().await;
    let p = w.create_profile(&old, true).await;
    let new = w.new_user().await;
    w.create_profile(&new, true).await;
    let org = w
        .module
        .create_organization(
            new.auth_account_id.clone(),
            CreateOrganizationDto {
                name: format!("l1race{}", kernel::generate_id()),
            },
        )
        .await
        .unwrap();
    let destination = AccountId::new(
        sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE nanoid=$1")
            .bind(&org.account_id)
            .fetch_one(w.pool())
            .await
            .unwrap(),
    );
    let old_account = owner(&w, &p).await;
    sqlx::query("INSERT INTO organization_members(org_account_id,member_account_id,role,status,invited_by) SELECT $1,$2,'member','active',invited_by FROM organization_members WHERE org_account_id=$1 LIMIT 1").bind(destination.as_ref()).bind(old_account.as_ref()).execute(w.pool()).await.unwrap();
    let req = w
        .module
        .request_profile_transfer(&old.auth_account_id, p.nanoid.clone(), org.account_id)
        .await
        .unwrap();
    let before = register(&w, &old, &p).await;
    let id = before["id"].as_str().unwrap();
    // Stop point: hold the old owner's Account gate until all three operations are actually
    // queued on a lock, then release them together.
    let mut gate = w.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('emumet/account-lifecycle/' || $1::text, 0))")
        .bind(old_account.as_ref())
        .execute(&mut *gate)
        .await
        .unwrap();
    let (m1, m2, m3) = (w.module.clone(), w.module.clone(), w.module.clone());
    let (a1, a2, a3) = (
        old.auth_account_id.clone(),
        old.auth_account_id.clone(),
        new.auth_account_id.clone(),
    );
    let (n1, n2) = (p.nanoid.clone(), p.nanoid.clone());
    let link_id = id.to_owned();
    let registration = tokio::spawn(async move {
        m1.register_home_link(&a1, None, &n1, "https://home.example", 3)
            .await
    });
    let unlink = tokio::spawn(async move { m2.unlink_home_link(&a2, None, &n2, &link_id).await });
    let transfer =
        tokio::spawn(async move { m3.accept_profile_transfer_request(&a3, req.nanoid).await });
    wait_for_lock_waiters(&w, 3).await;
    gate.commit().await.unwrap();
    let (registration, unlink, transfer) = (
        registration.await.unwrap(),
        unlink.await.unwrap(),
        transfer.await.unwrap(),
    );
    transfer.unwrap();
    for result in [registration.map(|_| ()), unlink] {
        if let Err(e) = result {
            assert!(
                matches!(
                    e.current_context(),
                    kernel::KernelError::Concurrency | kernel::KernelError::PermissionDenied
                ),
                "unexpected race error: {e:?}"
            );
        }
    }
    assert_eq!(owner(&w, &p).await, destination);
    let rows = sqlx::query_as::<_, (i64, String, i64)>(
        "SELECT owner_account_id,state,generation FROM profile_links WHERE profile_id=$1",
    )
    .bind(p.id.as_ref())
    .fetch_all(w.pool())
    .await
    .unwrap();
    assert!(rows.iter().all(|r| r.0 == *destination.as_ref()));
    assert!(rows.iter().filter(|r| r.1 == "active").count() <= 1);
    let original = sqlx::query_as::<_, (String, i64, String)>(
        "SELECT state,generation,origin FROM profile_links WHERE id=$1",
    )
    .bind(id)
    .fetch_one(w.pool())
    .await
    .unwrap();
    assert_eq!(original.2, "https://home.example");
    assert!(matches!(
        (original.0.as_str(), original.1),
        ("active", 1) | ("unlinked", 2)
    ));
    assert!(w
        .module
        .list_home_links(&old.auth_account_id, None, &p.nanoid)
        .await
        .is_err());
}

#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_held_owner_can_inspect_and_unlink_but_cannot_register() {
    let w = world().await;
    let stranger = w.new_user().await;
    for event in [
        AccountEvent::Banned {
            reason: "L1 held-owner regression".into(),
            banned_at: time::OffsetDateTime::now_utc(),
        },
        AccountEvent::Suspended {
            reason: "L1 held-owner regression".into(),
            suspended_at: time::OffsetDateTime::now_utc(),
            expires_at: None,
        },
    ] {
        let u = w.new_user().await;
        let p = w.create_profile(&u, false).await;
        let link = register(&w, &u, &p).await;
        let base = format!("/api/v1/profiles/{}/links", p.nanoid);
        let delete = format!("{base}/{}", link["id"].as_str().unwrap());
        raw_event(&w, &owner(&w, &p).await, event).await;
        w.module.project_batch().await.unwrap();
        let (status, list) = request(&w, Some(&u), "GET", &base, None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["items"][0]["state"], "active");
        assert_eq!(list["items"][0]["generation"], 1);
        assert_eq!(
            request(
                &w,
                Some(&u),
                "POST",
                &base,
                Some(json!({"origin":"https://home.example","trust_mode":3})),
                None
            )
            .await
            .0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            request(&w, Some(&stranger), "DELETE", &delete, None, None)
                .await
                .0,
            StatusCode::FORBIDDEN
        );
        for _ in 0..2 {
            assert_eq!(
                request(&w, Some(&u), "DELETE", &delete, None, None).await.0,
                StatusCode::NO_CONTENT
            );
        }
        let (status, list) = request(&w, Some(&u), "GET", &base, None, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(list["items"][0]["state"], "unlinked");
        assert_eq!(list["items"][0]["generation"], 2);
    }
}

/// Poll `pg_locks` until some backend of this database is *actually waiting* on a lock (advisory,
/// row or transaction), not merely "has not finished yet".
async fn wait_for_lock_waiter(w: &GraphWorld) {
    wait_for_lock_waiters(w, 1).await
}
/// Like [`wait_for_lock_waiter`] but until at least `count` backends are waiting.
async fn wait_for_lock_waiters(w: &GraphWorld, count: i64) {
    // Generous: organisation Profile creation generates an RSA key (slow in debug/coverage builds)
    // before it reaches the gate. Only a failing test ever waits this long.
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_locks WHERE NOT granted AND database=(SELECT oid FROM pg_database WHERE datname=current_database())",
            )
            .fetch_one(w.pool())
            .await
            .unwrap();
            if waiting >= count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("concurrent operations never reached their lock waits");
}

/// Mixed organisation (low-ID Profile without a link, high-ID Profile with a link), physically
/// deleted by deactivation, then reactivated while the old deactivation event is still inside the
/// replay window. The shared lock order is Profile gates ascending -> relationship gates ascending:
/// the projector must queue on the *lowest* Profile gate without already holding the high
/// Profile's Profile/relationship gates.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_projector_reactivation_replay_locks_all_profiles_ascending_before_relationships()
{
    use application::service::session_context::OrganizationContext;
    use kernel::interfaces::event::EventApplier;
    use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
    use kernel::interfaces::repository::DependOnProfileRepository;
    use kernel::prelude::entity::OrgRole;
    use kernel::prelude::entity::{Nanoid, Profile, ProfileId};
    let w = world().await;
    let u = w.new_user().await;
    w.create_profile(&u, true).await;
    let org = w
        .module
        .create_organization(
            u.auth_account_id.clone(),
            CreateOrganizationDto {
                name: format!("l1order{}", kernel::generate_id()),
            },
        )
        .await
        .unwrap();
    let a = AccountId::new(
        sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE nanoid=$1")
            .bind(&org.account_id)
            .fetch_one(w.pool())
            .await
            .unwrap(),
    );
    let context = OrganizationContext {
        org_account_id: a.clone(),
        org_account_nanoid: org.account_id,
        role: OrgRole::Owner,
    };
    let mut ids = vec![];
    for linked in [false, true] {
        let mut c = w.module.database_connection().connection().await.unwrap();
        let event = w
            .module
            .profile_repository()
            .save(
                &mut c,
                Profile::create(
                    ProfileId::new(kernel::generate_id()),
                    a.clone(),
                    None,
                    None,
                    None,
                    None,
                    Nanoid::default(),
                ),
            )
            .await
            .unwrap();
        let mut profile = None;
        Profile::apply(&mut profile, event).unwrap();
        let profile = profile.unwrap();
        w.module
            .profile_read_model()
            .create(&mut c, &profile)
            .await
            .unwrap();
        drop(c);
        if linked {
            w.module
                .register_home_link(
                    &u.auth_account_id,
                    Some(&context),
                    profile.nanoid().as_ref(),
                    "https://home.example",
                    3,
                )
                .await
                .unwrap();
        }
        ids.push(*profile.id().as_ref());
    }
    let (low, high) = (ids[0], ids[1]);
    assert!(low < high);
    // Physically delete every Profile row (ledger-only identity remains for `high`).
    raw_event(&w, &a, AccountEvent::Deactivated).await;
    w.module.project_batch().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE account_id=$1")
            .bind(a.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap(),
        0
    );
    raw_event(&w, &a, AccountEvent::Reactivated).await;
    // Replay the window again: it still contains the old deactivation event.
    sqlx::query("DELETE FROM projection_checkpoints WHERE projector_name='account_projector'")
        .execute(w.pool())
        .await
        .unwrap();
    let mut holder = w.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('emumet/profile-lifecycle/' || $1::text, 0))")
        .bind(low)
        .execute(&mut *holder)
        .await
        .unwrap();
    let module = w.module.clone();
    let task = tokio::spawn(async move { module.project_batch().await });
    wait_for_lock_waiter(&w).await;
    // The projector queues on the lowest Profile gate and holds nothing for the high Profile.
    let mut probe = w.pool().begin().await.unwrap();
    for (label, key) in [
        ("profile gate", format!("emumet/profile-lifecycle/{high}")),
        (
            "relationship gate",
            format!("emumet.profile_relationship:{high}"),
        ),
    ] {
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))"
            )
            .bind(key)
            .fetch_one(&mut *probe)
            .await
            .unwrap(),
            "high-ID {label} was held while waiting on the lower Profile gate"
        );
    }
    probe.commit().await.unwrap();
    holder.commit().await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE account_id=$1")
            .bind(a.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap(),
        2
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM profile_links WHERE owner_account_id=$1 AND state='unlinked'"
        )
        .bind(a.as_ref())
        .fetch_one(w.pool())
        .await
        .unwrap(),
        1
    );
}

async fn new_org(
    w: &GraphWorld,
    u: &User,
) -> (
    AccountId,
    application::service::session_context::OrganizationContext,
) {
    use application::service::session_context::OrganizationContext;
    use kernel::prelude::entity::OrgRole;
    let org = w
        .module
        .create_organization(
            u.auth_account_id.clone(),
            CreateOrganizationDto {
                name: format!("l1gate{}", kernel::generate_id()),
            },
        )
        .await
        .unwrap();
    let a = AccountId::new(
        sqlx::query_scalar::<_, i64>("SELECT id FROM accounts WHERE nanoid=$1")
            .bind(&org.account_id)
            .fetch_one(w.pool())
            .await
            .unwrap(),
    );
    let context = OrganizationContext {
        org_account_id: a.clone(),
        org_account_nanoid: org.account_id,
        role: OrgRole::Owner,
    };
    (a, context)
}

/// Creates an organisation Profile row through the repositories **without** any lifecycle gate
/// (stand-in for an unguarded writer), using a pre-allocated ID.
async fn ungated_org_profile(w: &GraphWorld, a: &AccountId, id: i64) -> i64 {
    use kernel::interfaces::event::EventApplier;
    use kernel::interfaces::read_model::{DependOnProfileReadModel, ProfileReadModel};
    use kernel::interfaces::repository::DependOnProfileRepository;
    use kernel::prelude::entity::{Nanoid, Profile, ProfileId};
    let mut c = w.module.database_connection().connection().await.unwrap();
    let event = w
        .module
        .profile_repository()
        .save(
            &mut c,
            Profile::create(
                ProfileId::new(id),
                a.clone(),
                None,
                None,
                None,
                None,
                Nanoid::default(),
            ),
        )
        .await
        .unwrap();
    let mut profile = None;
    Profile::apply(&mut profile, event).unwrap();
    let profile = profile.unwrap();
    w.module
        .profile_read_model()
        .create(&mut c, &profile)
        .await
        .unwrap();
    *profile.id().as_ref()
}

/// Invariant: an Account's Profile set changes only under that Account's lifecycle gate. The
/// organisation-Profile creation API therefore queues on the Account gate before it writes
/// anything (a held gate = a lifecycle transaction that has settled its Profile set).
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_organization_profile_creation_waits_for_the_account_gate() {
    use application::dto::profile::CreateProfileDto;
    use kernel::interfaces::database::DependOnDatabaseConnection as _;
    let w = world().await;
    let u = w.new_user().await;
    w.create_profile(&u, true).await;
    let (a, context) = new_org(&w, &u).await;
    let count = || async {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE account_id=$1")
            .bind(a.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap()
    };
    let before = count().await;
    let mut gate = w.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('emumet/account-lifecycle/' || $1::text, 0))")
        .bind(a.as_ref())
        .execute(&mut *gate)
        .await
        .unwrap();
    let module = w.module.clone();
    let task = tokio::spawn(async move {
        use application::service::profile::CreateOrganizationProfileUseCase;
        module
            .create_organization_profile(
                context,
                CreateProfileDto {
                    display_name: None,
                    summary: None,
                    username: format!("gated{}", kernel::generate_id()),
                },
            )
            .await
    });
    wait_for_lock_waiter(&w).await;
    assert!(!task.is_finished());
    assert_eq!(
        count().await,
        before,
        "Profile row appeared outside the Account gate"
    );
    gate.commit().await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(count().await, before + 1);
    let _ = w.module.database_connection();
}

/// A Profile that appears **after** the lifecycle transaction settled its Profile set (here via a
/// writer that takes no gate at all) must never make the transaction take a Profile gate out of
/// order: it fails fast with `Concurrency`, rolls back (releasing the high Profile gate) and the
/// retry in a new transaction locks the complete set ascending.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_projector_retries_when_profile_set_grows_after_it_was_settled() {
    let w = world().await;
    let u = w.new_user().await;
    w.create_profile(&u, true).await;
    let (a, context) = new_org(&w, &u).await;
    let low = kernel::generate_id();
    let high = ungated_org_profile(&w, &a, kernel::generate_id()).await;
    assert!(low < high);
    let nanoid: String = sqlx::query_scalar("SELECT nanoid FROM profiles WHERE id=$1")
        .bind(high)
        .fetch_one(w.pool())
        .await
        .unwrap();
    w.module
        .register_home_link(
            &u.auth_account_id,
            Some(&context),
            &nanoid,
            "https://home.example",
            3,
        )
        .await
        .unwrap();
    raw_event(&w, &a, AccountEvent::Deactivated).await;
    let lock = |id: i64, ns: &'static str| {
        let sql = format!("SELECT pg_advisory_xact_lock(hashtextextended('{ns}' || $1::text, 0))");
        async move { (sql, id) }
    };
    let mut first = w.pool().begin().await.unwrap();
    let (sql, id) = lock(high, "emumet/profile-lifecycle/").await;
    sqlx::query(&sql)
        .bind(id)
        .execute(&mut *first)
        .await
        .unwrap();
    let module = w.module.clone();
    let task = tokio::spawn(async move { module.project_batch().await });
    // The projector holds the Account gate and has settled {high}; it waits for `high`.
    wait_for_lock_waiter(&w).await;
    ungated_org_profile(&w, &a, low).await;
    let mut second = w.pool().begin().await.unwrap();
    let (sql, id) = lock(low, "emumet/profile-lifecycle/").await;
    sqlx::query(&sql)
        .bind(id)
        .execute(&mut *second)
        .await
        .unwrap();
    first.commit().await.unwrap();
    // First attempt fails fast (out-of-order request) and releases `high`; the retry queues on
    // the lowest gate (`low`, held by `second`) without holding `high`.
    wait_for_lock_waiter(&w).await;
    let mut probe = w.pool().begin().await.unwrap();
    for (label, key) in [
        ("profile gate", format!("emumet/profile-lifecycle/{high}")),
        (
            "relationship gate",
            format!("emumet.profile_relationship:{high}"),
        ),
    ] {
        assert!(
            sqlx::query_scalar::<_, bool>(
                "SELECT pg_try_advisory_xact_lock(hashtextextended($1, 0))"
            )
            .bind(key)
            .fetch_one(&mut *probe)
            .await
            .unwrap(),
            "high-ID {label} was held while waiting on the lower Profile gate"
        );
    }
    probe.commit().await.unwrap();
    second.commit().await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE account_id=$1")
            .bind(a.as_ref())
            .fetch_one(w.pool())
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM profile_links WHERE owner_account_id=$1 AND state='unlinked'"
        )
        .bind(a.as_ref())
        .fetch_one(w.pool())
        .await
        .unwrap(),
        1
    );
}

/// The Profile projector's fresh materialization adds a row to an Account's Profile set, so it
/// runs in its own transaction under that Account's gate.
#[test_with::env(DATABASE_URL)]
#[tokio::test]
async fn home_link_profile_projector_materialization_waits_for_the_account_gate() {
    use application::projection::ProjectProfileBatch;
    let w = world().await;
    let u = w.new_user().await;
    w.create_profile(&u, true).await;
    let (a, _) = new_org(&w, &u).await;
    let id = ungated_org_profile(&w, &a, kernel::generate_id()).await;
    sqlx::query("DELETE FROM profiles WHERE id=$1")
        .bind(id)
        .execute(w.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM projection_checkpoints WHERE projector_name='profile_projector'")
        .execute(w.pool())
        .await
        .unwrap();
    let mut gate = w.pool().begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('emumet/account-lifecycle/' || $1::text, 0))")
        .bind(a.as_ref())
        .execute(&mut *gate)
        .await
        .unwrap();
    let module = w.module.clone();
    let task = tokio::spawn(async move { module.project_profile_batch().await });
    wait_for_lock_waiter(&w).await;
    assert!(!task.is_finished());
    let rows = |id: i64| {
        let pool = w.pool().clone();
        async move {
            sqlx::query_scalar::<_, i64>("SELECT count(*) FROM profiles WHERE id=$1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(
        rows(id).await,
        0,
        "row materialized outside the Account gate"
    );
    gate.commit().await.unwrap();
    task.await.unwrap().unwrap();
    assert_eq!(rows(id).await, 1);
}
