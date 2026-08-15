use error_stack::{Report, ResultExt};
use kernel::interfaces::permission::{
    InstanceRole, PermissionChecker, PermissionReq, PermissionWriter, RelationTarget,
};
use kernel::prelude::entity::AuthAccountId;
use kernel::KernelError;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
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

#[derive(Debug, Serialize, Deserialize)]
struct RelationTuple {
    namespace: String,
    object: String,
    relation: String,
    subject_id: String,
}

#[derive(Debug, Deserialize)]
struct ListRelationTuplesResponse {
    relation_tuples: Vec<RelationTuple>,
    next_page_token: String,
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

    async fn list_instance_roles(
        &self,
        subject: &AuthAccountId,
    ) -> error_stack::Result<Vec<InstanceRole>, KernelError> {
        let subject_id = subject.as_ref().to_string();
        let mut relations: Vec<String> = Vec::new();
        let mut page_token = String::new();

        loop {
            let mut query = vec![
                ("namespace", "Instance"),
                ("object", "singleton"),
                ("subject_id", subject_id.as_str()),
            ];
            if !page_token.is_empty() {
                query.push(("page_token", page_token.as_str()));
            }

            let response = self
                .http_client
                .get(format!("{}/relation-tuples", self.read_url))
                .query(&query)
                .send()
                .await
                .change_context_lazy(|| KernelError::Internal)
                .attach_printable("Failed to list relation tuples from Keto")?;

            let status = response.status();

            if !status.is_success() {
                return Err(Report::new(KernelError::Internal)
                    .attach_printable(format!("Keto returned unexpected status: {}", status)));
            }

            let list: ListRelationTuplesResponse = response
                .json()
                .await
                .change_context_lazy(|| KernelError::Internal)
                .attach_printable("Failed to parse Keto relation-tuples response")?;

            relations.extend(
                list.relation_tuples
                    .into_iter()
                    .filter(|tuple| tuple.namespace == "Instance" && tuple.object == "singleton")
                    .map(|tuple| tuple.relation),
            );

            if list.next_page_token.is_empty() {
                break;
            }
            page_token = list.next_page_token;
        }

        Ok([InstanceRole::Admin, InstanceRole::Moderator]
            .into_iter()
            .filter(|role| relations.iter().any(|relation| relation == role.as_str()))
            .collect())
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

        let response = self
            .http_client
            .put(format!("{}/admin/relation-tuples", self.write_url))
            .json(&tuple)
            .send()
            .await
            .change_context_lazy(|| KernelError::Internal)
            .attach_printable("Failed to create relation tuple in Keto")?;
        if response.status() == reqwest::StatusCode::CONFLICT {
            return Ok(());
        }
        response.error_for_status().map_err(|e| {
            Report::new(KernelError::Internal).attach_printable(format!("Keto write error: {e}"))
        })?;

        Ok(())
    }

    async fn delete_relation(
        &self,
        target: &RelationTarget,
        subject: &AuthAccountId,
    ) -> error_stack::Result<(), KernelError> {
        let response = self
            .http_client
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
            .attach_printable("Failed to delete relation tuple from Keto")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        response.error_for_status().map_err(|e| {
            Report::new(KernelError::Internal).attach_printable(format!("Keto delete error: {e}"))
        })?;

        Ok(())
    }
}

#[cfg(test)]
#[path = "keto/tests.rs"]
mod tests;
