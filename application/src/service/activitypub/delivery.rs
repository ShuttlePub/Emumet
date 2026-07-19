use super::fetch::{client_for_url, validate_fetch_url};
use super::ACTIVITY_JSON;
use base64::{engine::general_purpose, Engine as _};
use error_stack::Report;
use kernel::activitypub::Activity;
use kernel::interfaces::crypto::{KeyEncryptor, PasswordProvider};
use kernel::interfaces::database::DatabaseConnection;
use kernel::interfaces::http_signing::{HttpSigner, HttpSigningRequest};
use kernel::interfaces::repository::SigningKeyRepository;
use kernel::prelude::entity::AccountId;
use kernel::KernelError;
use reqwest::header::{CONTENT_TYPE, DATE, HOST};
use sha2::Digest;

fn host_header(url: &reqwest::Url) -> error_stack::Result<String, KernelError> {
    let host = url.host_str().ok_or_else(|| {
        Report::new(KernelError::Rejected).attach_printable("URL host is missing")
    })?;
    Ok(match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    })
}

pub(super) async fn deliver_activity_to_inbox<D, S>(
    database_connection: &D,
    signing_key_repository: &S,
    password_provider: &impl PasswordProvider,
    key_encryptor: &impl KeyEncryptor,
    http_signer: &impl HttpSigner,
    account_id: &AccountId,
    inbox_url: &str,
    activity: &Activity,
    activity_name: &str,
) -> error_stack::Result<(), KernelError>
where
    D: DatabaseConnection,
    S: SigningKeyRepository<Executor = D::Executor>,
{
    let body = serde_json::to_vec(activity).map_err(|e| {
        Report::from(e)
            .change_context(KernelError::Internal)
            .attach_printable(format!("Failed to serialize {activity_name} activity"))
    })?;
    let url = reqwest::Url::parse(inbox_url).map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("Remote inbox URL is invalid: {e}"))
    })?;
    let resolved_addresses = validate_fetch_url(&url).await?;
    let host = host_header(&url)?;
    let digest = format!(
        "SHA-256={}",
        general_purpose::STANDARD.encode(sha2::Sha256::digest(&body))
    );
    let date = httpdate::fmt_http_date(std::time::SystemTime::now());
    let mut headers = std::collections::HashMap::new();
    headers.insert("host".to_string(), host.clone());
    headers.insert("date".to_string(), date.clone());
    headers.insert("digest".to_string(), digest.clone());
    headers.insert("content-type".to_string(), ACTIVITY_JSON.to_string());

    let signing_request = HttpSigningRequest {
        method: "POST".to_string(),
        url: inbox_url.to_string(),
        headers,
        body: Some(body.clone()),
    };
    let mut executor = database_connection.get_executor().await?;
    let signing_key = signing_key_repository
        .find_active_by_account_id(&mut executor, account_id)
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| {
            Report::new(KernelError::NotFound)
                .attach_printable("No active signing key found for account")
        })?;
    let password = password_provider.get_password()?;
    let private_key_pem = key_encryptor.decrypt(signing_key.encrypted_private_key(), &password)?;
    let signature = http_signer
        .sign(
            &signing_request,
            &private_key_pem,
            &signing_key.key_id_uri,
            signing_key.algorithm(),
        )
        .await?;

    let client = client_for_url(&url, &resolved_addresses)?;
    let mut request = client
        .post(url)
        .header(HOST, host)
        .header(DATE, date)
        .header("Digest", digest)
        .header(CONTENT_TYPE, ACTIVITY_JSON)
        .body(body);
    for (name, value) in &signature.cavage_headers {
        // Skip headers already set explicitly above to avoid duplicates.
        // nginx returns 400 for duplicate Host headers.
        let lower = name.to_ascii_lowercase();
        if lower == "host" || lower == "date" || lower == "digest" || lower == "content-type" {
            continue;
        }
        request = request.header(name.as_str(), value.as_str());
    }
    let response = request.send().await.map_err(|e| {
        Report::new(KernelError::Rejected)
            .attach_printable(format!("{activity_name} delivery failed: {e}"))
    })?;
    if !response.status().is_success() {
        return Err(Report::new(KernelError::Rejected).attach_printable(format!(
            "{activity_name} delivery returned {}",
            response.status()
        )));
    }
    Ok(())
}
