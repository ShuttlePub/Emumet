use error_stack::{Report, ResultExt};
use kernel::interfaces::permission::{
    PermissionChecker, PermissionReq, PermissionWriter, Relation, Resource,
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
        let subject_id = subject.as_ref().to_string();

        for relation in req.relations() {
            let body = CheckRequest {
                namespace: req.resource().namespace().to_string(),
                object: req.resource().object_id(),
                relation: relation.as_str().to_string(),
                subject_id: subject_id.clone(),
            };

            let response = self
                .http_client
                .post(format!("{}/relation-tuples/check", self.read_url))
                .json(&body)
                .send()
                .await
                .change_context_lazy(|| KernelError::Internal)
                .attach_printable("Failed to check permission with Keto")?;

            if response.status().is_success() {
                let check: CheckResponse = response
                    .json()
                    .await
                    .change_context_lazy(|| KernelError::Internal)
                    .attach_printable("Failed to parse Keto check response")?;

                if check.allowed {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

impl PermissionWriter for KetoClient {
    async fn create_relation(
        &self,
        resource: &Resource,
        relation: Relation,
        subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let tuple = RelationTuple {
            namespace: resource.namespace().to_string(),
            object: resource.object_id(),
            relation: relation.as_str().to_string(),
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
        resource: &Resource,
        relation: Relation,
        subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        self.http_client
            .delete(format!("{}/admin/relation-tuples", self.write_url))
            .query(&[
                ("namespace", resource.namespace()),
                ("object", &resource.object_id()),
                ("relation", relation.as_str()),
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
