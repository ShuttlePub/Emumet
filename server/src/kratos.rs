use reqwest::Client;
use serde::Deserialize;

/// Kratos public API で registration flow の実在と **実際の期限** を確認する (登録前 email challenge を実 flow
/// に束縛し、その期限以内にするため)。
///
/// `GET /self-service/registration/flows?id=` は browser flow だと CSRF cookie を要求し、cookie なしでは
/// 403 になる (Kratos v1.3.1)。そのため呼び出し側が受け取ったブラウザの CSRF cookie を転送して照会する。
/// cookie がなく 403 になった flow は期限を確認できないので **使えない (`NotUsable`) とし、期限を推測しない**
/// (fail-closed)。最終的な flow 束縛の強制は hook が Kratos の実 context と proof を照合する側 (E15d)。
#[derive(Clone)]
pub struct KratosRegistrationFlowDirectory {
    public_url: String,
    http_client: Client,
}

const FLOW_LOOKUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Deserialize)]
struct KratosFlowBody {
    expires_at: Option<String>,
}

impl KratosRegistrationFlowDirectory {
    pub fn new(public_url: &str) -> Self {
        Self {
            public_url: public_url.trim_end_matches('/').to_string(),
            http_client: Client::builder()
                .timeout(FLOW_LOOKUP_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
        }
    }
}

impl kernel::interfaces::repository::RegistrationFlowDirectory for KratosRegistrationFlowDirectory {
    async fn lookup(
        &self,
        flow_id: &str,
        browser_cookie: Option<&str>,
    ) -> error_stack::Result<
        kernel::interfaces::repository::RegistrationFlowStatus,
        kernel::KernelError,
    > {
        use kernel::interfaces::repository::RegistrationFlowStatus;
        use kernel::KernelError;
        let mut request = self
            .http_client
            .get(format!(
                "{}/self-service/registration/flows",
                self.public_url
            ))
            .query(&[("id", flow_id)])
            .header("accept", "application/json");
        if let Some(cookie) = browser_cookie {
            request = request.header("cookie", cookie);
        }
        let response = request.send().await.map_err(|error| {
            let kind = if error.is_timeout() {
                KernelError::Timeout
            } else {
                KernelError::Internal
            };
            error_stack::Report::new(kind)
        })?;
        let status = response.status();
        let now = time::OffsetDateTime::now_utc();
        match status.as_u16() {
            200 => {
                let body: KratosFlowBody = response
                    .json()
                    .await
                    .map_err(|_| error_stack::Report::new(KernelError::Internal))?;
                let expires_at = body
                    .expires_at
                    .as_deref()
                    .and_then(|value| {
                        time::OffsetDateTime::parse(
                            value,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .ok()
                    })
                    .ok_or_else(|| error_stack::Report::new(KernelError::Internal))?;
                Ok(if expires_at > now {
                    RegistrationFlowStatus::Active { expires_at }
                } else {
                    RegistrationFlowStatus::NotUsable
                })
            }
            // CSRF cookie がない/不一致の browser flow は中身 (期限) を確認できないので使わない。
            403 | 404 | 410 => Ok(RegistrationFlowStatus::NotUsable),
            _ => Err(error_stack::Report::new(KernelError::Internal)),
        }
    }
}
