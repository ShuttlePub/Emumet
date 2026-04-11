use std::collections::HashSet;

use reqwest::header::{AUTHORIZATION, COOKIE, LOCATION, SET_COOKIE};
use reqwest::{Client, StatusCode};
use serde_json::json;
use url::Url;

const HYDRA_ADMIN: &str = "http://localhost:4445";
const HYDRA_PUBLIC: &str = "http://localhost:4444";
const KRATOS_PUBLIC: &str = "http://localhost:4433";
const REDIRECT_URI: &str = "http://localhost:9999/callback";
const CLIENT_ID: &str = "e2e-test-client";
const CLIENT_SECRET: &str = "e2e-test-secret";

pub async fn get_jwt_for_test_user() -> String {
    ensure_hydra_client_exists().await;
    let session_cookie = kratos_browser_login().await;
    let code = hydra_oauth2_flow(&session_cookie).await;
    exchange_code_for_token(&code).await
}

async fn ensure_hydra_client_exists() {
    let client = Client::new();
    let payload = json!({
        "client_id": CLIENT_ID,
        "client_secret": CLIENT_SECRET,
        "grant_types": ["authorization_code"],
        "response_types": ["code"],
        "redirect_uris": [REDIRECT_URI],
        "scope": "openid offline",
        "token_endpoint_auth_method": "client_secret_basic",
        "audience": ["account"],
        "skip_consent": true
    });

    let response = client
        .post(format!("{HYDRA_ADMIN}/admin/clients"))
        .json(&payload)
        .send()
        .await
        .expect("failed to call hydra admin for client creation");

    if response.status() == StatusCode::CREATED || response.status() == StatusCode::CONFLICT {
        return;
    }

    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|_| "<failed to read body>".to_string());
    panic!(
        "failed to ensure hydra client exists: status={}, body={body}",
        status
    );
}

async fn kratos_browser_login() -> String {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build reqwest client");

    let init_response = client
        .get(format!("{KRATOS_PUBLIC}/self-service/login/browser"))
        .header("accept", "application/json")
        .send()
        .await
        .expect("failed to initialize kratos browser login flow");

    let csrf_cookie = extract_csrf_cookie(&init_response);

    let flow: serde_json::Value = init_response
        .json()
        .await
        .expect("failed to parse kratos browser login flow");

    let flow_id = flow
        .get("id")
        .and_then(|v| v.as_str())
        .expect("kratos browser login flow missing id");

    let csrf_token = flow["ui"]["nodes"]
        .as_array()
        .and_then(|nodes| {
            nodes.iter().find_map(|node| {
                let attrs = &node["attributes"];
                if attrs["name"].as_str() == Some("csrf_token") {
                    attrs["value"].as_str().map(String::from)
                } else {
                    None
                }
            })
        })
        .expect("kratos browser login flow missing csrf_token in UI nodes");

    let login_response = client
        .post(format!("{KRATOS_PUBLIC}/self-service/login?flow={flow_id}"))
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("cookie", &csrf_cookie)
        .json(&json!({
            "method": "password",
            "identifier": "testuser@example.com",
            "password": "testuser",
            "csrf_token": csrf_token
        }))
        .send()
        .await
        .expect("failed to submit kratos browser login");

    let status = login_response.status();
    assert!(
        status.is_success(),
        "kratos browser login failed: status={status}"
    );

    login_response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|cookie_str| {
            if cookie_str.starts_with("ory_kratos_session=") {
                Some(
                    cookie_str
                        .split(';')
                        .next()
                        .unwrap()
                        .trim_start_matches("ory_kratos_session=")
                        .to_string(),
                )
            } else {
                None
            }
        })
        .expect("login response did not set ory_kratos_session cookie")
}

fn extract_csrf_cookie(response: &reqwest::Response) -> String {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|cookie_str| {
            if cookie_str.starts_with("csrf_token") {
                Some(cookie_str.split(';').next().unwrap().to_string())
            } else {
                None
            }
        })
        .expect("kratos browser login flow did not return a csrf_token cookie")
}

async fn hydra_oauth2_flow(session_cookie: &str) -> String {
    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("failed to build reqwest client");

    let auth_url = format!(
        "{HYDRA_PUBLIC}/oauth2/auth?client_id={CLIENT_ID}&response_type=code&scope=openid&redirect_uri={REDIRECT_URI}&audience=account&state=e2e-state"
    );

    let mut next_url = auth_url;
    let mut seen = HashSet::new();
    let mut cookie_jar: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    for _ in 0..10 {
        let parsed = Url::parse(&next_url).expect("invalid redirect url in oauth2 flow");

        if parsed.host_str() == Some("localhost") && parsed.port_or_known_default() == Some(9999) {
            let code = parsed
                .query_pairs()
                .find(|(k, _)| k == "code")
                .map(|(_, v)| v.to_string())
                .unwrap_or_else(|| panic!("callback did not include authorization code: {parsed}"));
            return code;
        }

        if !seen.insert(next_url.clone()) {
            panic!("oauth2 redirect loop detected at {next_url}");
        }

        let host_key = format!(
            "{}:{}",
            parsed.host_str().unwrap_or(""),
            parsed.port_or_known_default().unwrap_or(80)
        );

        let mut cookies: Vec<String> = cookie_jar.get(&host_key).cloned().unwrap_or_default();

        if parsed.host_str() == Some("localhost") && parsed.port_or_known_default() == Some(8080) {
            cookies.push(format!("ory_kratos_session={session_cookie}"));
        }

        let mut request = client.get(&next_url);
        if !cookies.is_empty() {
            request = request.header(COOKIE, cookies.join("; "));
        }

        let response = request.send().await.expect("oauth2 flow request failed");

        for set_cookie in response.headers().get_all(SET_COOKIE).iter() {
            if let Ok(val) = set_cookie.to_str() {
                let name_value = val.split(';').next().unwrap().to_string();
                let jar = cookie_jar.entry(host_key.clone()).or_default();
                let name = name_value.split('=').next().unwrap();
                jar.retain(|c| !c.starts_with(&format!("{name}=")));
                jar.push(name_value);
            }
        }

        if !response.status().is_redirection() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read body>".to_string());
            panic!("oauth2 flow expected redirect, got status={status}, body={body}");
        }

        let location = response
            .headers()
            .get(LOCATION)
            .and_then(|v| v.to_str().ok())
            .expect("redirect response missing location header");

        next_url = if location.starts_with("http://") || location.starts_with("https://") {
            location.to_string()
        } else {
            parsed
                .join(location)
                .expect("failed to join relative redirect")
                .to_string()
        };
    }

    panic!("oauth2 flow exceeded max redirect hops");
}

async fn exchange_code_for_token(code: &str) -> String {
    let client = Client::new();
    let basic = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        format!("{CLIENT_ID}:{CLIENT_SECRET}"),
    );

    let response: serde_json::Value = client
        .post(format!("{HYDRA_PUBLIC}/oauth2/token"))
        .header(AUTHORIZATION, format!("Basic {basic}"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
        ])
        .send()
        .await
        .expect("failed to call hydra token endpoint")
        .error_for_status()
        .expect("hydra token exchange failed")
        .json()
        .await
        .expect("failed to parse hydra token response");

    response
        .get("access_token")
        .and_then(|v| v.as_str())
        .expect("token response missing access_token")
        .to_string()
}
