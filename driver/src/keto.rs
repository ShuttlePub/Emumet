use error_stack::{Report, ResultExt};
use kernel::interfaces::permission::{
    PermissionChecker, PermissionReq, PermissionWriter, RelationTarget,
};
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use reqwest::Client;
use serde::{Deserialize, Serialize};

pub struct KetoClient {
    read_url: String,
    write_url: String,
    http_client: Client,
}

impl KetoClient {
    pub fn new(read_url: String, write_url: String) -> Self {
        let read_url = read_url.trim_end_matches('/').to_string();
        let write_url = write_url.trim_end_matches('/').to_string();
        Self {
            read_url,
            write_url,
            http_client: Client::new(),
        }
    }
}

#[derive(Debug, Serialize)]
struct CheckRequest {
    namespace: String,
    object: String,
    relation: String,
    subject_id: String,
}

#[derive(Debug, Deserialize)]
struct CheckResponse {
    allowed: bool,
}

#[derive(Debug, Serialize)]
struct RelationTuple {
    namespace: String,
    object: String,
    relation: String,
    subject_id: String,
}

impl PermissionChecker for KetoClient {
    async fn check(
        &self,
        subject: &AuthAccountId,
        req: &PermissionReq,
    ) -> error_stack::Result<bool, KernelError> {
        let body = CheckRequest {
            namespace: req.namespace().to_string(),
            object: req.object_id(),
            relation: req.permission_name().to_string(),
            subject_id: subject.as_ref().to_string(),
        };

        let response = self
            .http_client
            .post(format!("{}/relation-tuples/check", self.read_url))
            .json(&body)
            .send()
            .await
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable("Failed to check permission with Keto")?;

        let status = response.status();

        // Keto v0.12 returns 403 for "not allowed" — treat as allowed=false
        if status == reqwest::StatusCode::FORBIDDEN {
            return Ok(false);
        }

        if !status.is_success() {
            return Err(Report::new(KernelError::Internal)
                .attach_printable(format!("Keto returned unexpected status: {}", status)));
        }

        let check: CheckResponse = response
            .json()
            .await
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable("Failed to parse Keto check response")?;

        Ok(check.allowed)
    }
}

impl PermissionWriter for KetoClient {
    async fn create_relation(
        &self,
        target: &RelationTarget,
        subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let tuple = RelationTuple {
            namespace: target.namespace().to_string(),
            object: target.object_id(),
            relation: target.relation_str().to_string(),
            subject_id: subject.as_ref().to_string(),
        };

        self.http_client
            .put(format!("{}/admin/relation-tuples", self.write_url))
            .json(&tuple)
            .send()
            .await
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable("Failed to create relation tuple in Keto")?
            .error_for_status()
            .map_err(|e| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Keto write error: {e}"))
            })?;

        Ok(())
    }

    async fn delete_relation(
        &self,
        target: &RelationTarget,
        subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.http_client
            .delete(format!("{}/admin/relation-tuples", self.write_url))
            .query(&[
                ("namespace", target.namespace()),
                ("object", &target.object_id()),
                ("relation", target.relation_str()),
                ("subject_id", &subject.as_ref().to_string()),
            ])
            .send()
            .await
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable("Failed to delete relation tuple from Keto")?
            .error_for_status()
            .map_err(|e| {
                Report::new(KernelError::Internal)
                    .attach_printable(format!("Keto delete error: {e}"))
            })?;

        Ok(())
    }
}
