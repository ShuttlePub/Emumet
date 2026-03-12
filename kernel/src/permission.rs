use crate::entity::{AccountId, AuthAccountId};
use crate::KernelError;
use std::collections::HashSet;
use std::future::Future;
use std::ops::Add;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Relation {
    Owner,
    Editor,
    Signer,
    Admin,
    Moderator,
}

impl Relation {
    pub fn as_str(&self) -> &'static str {
        match self {
            Relation::Owner => "owner",
            Relation::Editor => "editor",
            Relation::Signer => "signer",
            Relation::Admin => "admin",
            Relation::Moderator => "moderator",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resource {
    Account(AccountId),
    Instance,
}

impl Resource {
    pub fn namespace(&self) -> &'static str {
        match self {
            Resource::Account(_) => "accounts",
            Resource::Instance => "instance",
        }
    }

    pub fn object_id(&self) -> String {
        match self {
            Resource::Account(id) => id.as_ref().to_string(),
            Resource::Instance => "singleton".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PermissionReq {
    resource: Resource,
    relations: HashSet<Relation>,
}

impl PermissionReq {
    pub fn new(resource: Resource, relations: impl IntoIterator<Item = Relation>) -> Self {
        Self {
            resource,
            relations: relations.into_iter().collect(),
        }
    }

    pub fn resource(&self) -> &Resource {
        &self.resource
    }

    pub fn relations(&self) -> &HashSet<Relation> {
        &self.relations
    }
}

#[derive(Debug, Clone)]
pub struct Permission(Vec<PermissionReq>);

impl Permission {
    pub fn new(req: PermissionReq) -> Self {
        Self(vec![req])
    }

    pub fn all(reqs: Vec<PermissionReq>) -> Self {
        Self(reqs)
    }

    pub fn requirements(&self) -> &[PermissionReq] {
        &self.0
    }
}

impl Add for Permission {
    type Output = Permission;

    fn add(mut self, rhs: Self) -> Self::Output {
        self.0.extend(rhs.0);
        self
    }
}

pub trait PermissionChecker: Send + Sync + 'static {
    fn check(
        &self,
        subject: &AuthAccountId,
        req: &PermissionReq,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send;

    fn satisfies(
        &self,
        subject: &AuthAccountId,
        permission: &Permission,
    ) -> impl Future<Output = error_stack::Result<bool, KernelError>> + Send {
        async move {
            for req in permission.requirements() {
                if !self.check(subject, req).await? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

pub trait DependOnPermissionChecker: Send + Sync {
    type PermissionChecker: PermissionChecker;
    fn permission_checker(&self) -> &Self::PermissionChecker;
}

pub trait PermissionWriter: Send + Sync + 'static {
    fn create_relation(
        &self,
        resource: &Resource,
        relation: Relation,
        subject: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete_relation(
        &self,
        resource: &Resource,
        relation: Relation,
        subject: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnPermissionWriter: Send + Sync {
    type PermissionWriter: PermissionWriter;
    fn permission_writer(&self) -> &Self::PermissionWriter;
}
