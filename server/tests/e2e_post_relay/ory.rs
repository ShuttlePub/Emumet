//! Real Kratos + Hydra authentication (browser login flow -> Hydra authorization code -> JWT).
use super::stack::*;
use reqwest::header::{AUTHORIZATION, COOKIE, LOCATION, SET_COOKIE};
use reqwest::{Client, StatusCode};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use url::Url;

const REDIRECT_URI: &str = "http://127.0.0.1:9999/callback";
const CLIENT_ID: &str = "t1-e2e-client";
const CLIENT_SECRET: &str = "t1-e2e-secret";
pub const PASSWORD: &str = "T1-e2e-Password-9137!";

#[derive(Clone)]
pub struct User {
    pub email: String,
    pub jwt: String,
}

pub fn email(label: &str) -> String {
    format!("{label}-{}@t1-e2e.example", uuid::Uuid::new_v4().simple())
}

/// Creates the identity with the Kratos admin API (the registration flow is a separate, already
/// covered concern) and then authenticates it through the real browser login + Hydra flow.
pub async fn login_new_user(label: &str) -> User {
    let email = email(label);
    create_identity(&email).await;
    let jwt = get_jwt(&email).await;
    User { email, jwt }
}

pub async fn create_identity(email: &str) {
    let response = Client::new()
        .post(format!("{KRATOS_ADMIN}/admin/identities"))
        .json(&json!({
            "schema_id": "default",
            "traits": {"email": email},
            "credentials": {"password": {"config": {"password": PASSWORD}}}
        }))
        .send()
        .await
        .expect("Kratos admin unreachable");
    assert!(
        response.status() == StatusCode::CREATED,
        "identity creation failed: {} {}",
        response.status(),
        response.text().await.unwrap_or_default()
    );
}

pub async fn get_jwt(email: &str) -> String {
    ensure_client().await;
    let session = kratos_login(email).await;
    let code = hydra_flow(&session).await;
    exchange(&code).await
}

async fn ensure_client() {
    let response = Client::new()
        .post(format!("{HYDRA_ADMIN}/admin/clients"))
        .json(&json!({
            "client_id": CLIENT_ID,
            "client_secret": CLIENT_SECRET,
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "redirect_uris": [REDIRECT_URI],
            "scope": "openid offline",
            "token_endpoint_auth_method": "client_secret_basic",
            "audience": ["account"],
            "skip_consent": true
        }))
        .send()
        .await
        .expect("Hydra admin unreachable");
    assert!(
        matches!(response.status(), StatusCode::CREATED | StatusCode::CONFLICT),
        "Hydra client creation failed: {}",
        response.status()
    );
}

async fn kratos_login(email: &str) -> String {
    let client = Client::new();
    let init = client
        .get(format!("{KRATOS_PUBLIC}/self-service/login/browser"))
        .header("accept", "application/json")
        .send()
        .await
        .expect("Kratos login flow init failed");
    let csrf_cookie = init
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|c| c.starts_with("csrf_token"))
        .map(|c| c.split(';').next().unwrap().to_string())
        .expect("no csrf cookie");
    let flow: serde_json::Value = init.json().await.unwrap();
    let id = flow["id"].as_str().expect("flow id");
    let csrf = flow["ui"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|n| {
            (n["attributes"]["name"] == "csrf_token")
                .then(|| n["attributes"]["value"].as_str().unwrap().to_string())
        })
        .expect("csrf node");
    let login = client
        .post(format!("{KRATOS_PUBLIC}/self-service/login?flow={id}"))
        .header("accept", "application/json")
        .header("cookie", &csrf_cookie)
        .json(&json!({"method":"password","identifier":email,"password":PASSWORD,"csrf_token":csrf}))
        .send()
        .await
        .expect("Kratos login failed");
    assert!(login.status().is_success(), "Kratos login status {}", login.status());
    login
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| {
            c.starts_with("ory_kratos_session=")
                .then(|| c.split(';').next().unwrap().trim_start_matches("ory_kratos_session=").to_string())
        })
        .expect("no Kratos session cookie")
}

async fn hydra_flow(session: &str) -> String {
    let client = Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let mut next = format!(
        "{HYDRA_PUBLIC}/oauth2/auth?client_id={CLIENT_ID}&response_type=code&scope=openid&redirect_uri={REDIRECT_URI}&audience=account&state=t1-state"
    );
    let mut seen = HashSet::new();
    let mut jar: HashMap<String, Vec<String>> = HashMap::new();
    for _ in 0..12 {
        let url = Url::parse(&next).unwrap();
        if url.port() == Some(9999) {
            return url
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| panic!("no authorization code in {url}"));
        }
        assert!(seen.insert(next.clone()), "redirect loop at {next}");
        let host = format!("{}:{}", url.host_str().unwrap(), url.port_or_known_default().unwrap());
        let mut cookies = jar.get(&host).cloned().unwrap_or_default();
        if host == LOGIN_ADDR {
            cookies.push(format!("ory_kratos_session={session}"));
        }
        let mut request = client.get(&next);
        if !cookies.is_empty() {
            request = request.header(COOKIE, cookies.join("; "));
        }
        let response = request.send().await.expect("oauth2 hop failed");
        for set in response.headers().get_all(SET_COOKIE).iter() {
            if let Ok(value) = set.to_str() {
                let pair = value.split(';').next().unwrap().to_string();
                let name = pair.split('=').next().unwrap().to_string();
                let cookies = jar.entry(host.clone()).or_default();
                cookies.retain(|c| !c.starts_with(&format!("{name}=")));
                cookies.push(pair);
            }
        }
        assert!(response.status().is_redirection(), "oauth2 hop status {}", response.status());
        let location = response.headers().get(LOCATION).unwrap().to_str().unwrap().to_string();
        next = if location.starts_with("http") { location } else { url.join(&location).unwrap().to_string() };
    }
    panic!("oauth2 flow exceeded hop limit");
}

async fn exchange(code: &str) -> String {
    let basic = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, format!("{CLIENT_ID}:{CLIENT_SECRET}"));
    let response: serde_json::Value = Client::new()
        .post(format!("{HYDRA_PUBLIC}/oauth2/token"))
        .header(AUTHORIZATION, format!("Basic {basic}"))
        .form(&[("grant_type", "authorization_code"), ("code", code), ("redirect_uri", REDIRECT_URI)])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .expect("Hydra token exchange failed")
        .json()
        .await
        .unwrap();
    response["access_token"].as_str().expect("access_token").to_string()
}
