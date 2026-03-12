use crate::entity::{AccountId, AuthAccountId};
use crate::KernelError;
use std::collections::HashSet;
use std::future::Future;
use std::ops::Add;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AccountRelation {
    Owner,
    Editor,
    Signer,
}

impl AccountRelation {
    pub fn as_str(&self) -> &'static str {
        match self {
            AccountRelation::Owner => "owner",
            AccountRelation::Editor => "editor",
            AccountRelation::Signer => "signer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstanceRole {
    Admin,
    Moderator,
}

impl InstanceRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            InstanceRole::Admin => "admin",
            InstanceRole::Moderator => "moderator",
        }
    }
}

const ACCOUNT_NAMESPACE: &str = "accounts";
const INSTANCE_NAMESPACE: &str = "instance";
const INSTANCE_OBJECT_ID: &str = "singleton";

#[derive(Debug, Clone)]
pub enum PermissionReq {
    Account {
        account_id: AccountId,
        relations: HashSet<AccountRelation>,
    },
    Instance {
        roles: HashSet<InstanceRole>,
    },
}

impl PermissionReq {
    pub fn account(
        account_id: AccountId,
        relations: impl IntoIterator<Item = AccountRelation>,
    ) -> Self {
        Self::Account {
            account_id,
            relations: relations.into_iter().collect(),
        }
    }

    pub fn instance(roles: impl IntoIterator<Item = InstanceRole>) -> Self {
        Self::Instance {
            roles: roles.into_iter().collect(),
        }
    }

    pub fn namespace(&self) -> &'static str {
        match self {
            PermissionReq::Account { .. } => ACCOUNT_NAMESPACE,
            PermissionReq::Instance { .. } => INSTANCE_NAMESPACE,
        }
    }

    pub fn object_id(&self) -> String {
        match self {
            PermissionReq::Account { account_id, .. } => account_id.as_ref().to_string(),
            PermissionReq::Instance { .. } => INSTANCE_OBJECT_ID.to_string(),
        }
    }

    pub fn relation_strs(&self) -> Vec<&'static str> {
        match self {
            PermissionReq::Account { relations, .. } => {
                relations.iter().map(|r| r.as_str()).collect()
            }
            PermissionReq::Instance { roles, .. } => roles.iter().map(|r| r.as_str()).collect(),
        }
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

#[derive(Debug, Clone)]
pub enum RelationTarget {
    Account {
        account_id: AccountId,
        relation: AccountRelation,
    },
    Instance {
        role: InstanceRole,
    },
}

impl RelationTarget {
    pub fn namespace(&self) -> &'static str {
        match self {
            RelationTarget::Account { .. } => ACCOUNT_NAMESPACE,
            RelationTarget::Instance { .. } => INSTANCE_NAMESPACE,
        }
    }

    pub fn object_id(&self) -> String {
        match self {
            RelationTarget::Account { account_id, .. } => account_id.as_ref().to_string(),
            RelationTarget::Instance { .. } => INSTANCE_OBJECT_ID.to_string(),
        }
    }

    pub fn relation_str(&self) -> &'static str {
        match self {
            RelationTarget::Account { relation, .. } => relation.as_str(),
            RelationTarget::Instance { role, .. } => role.as_str(),
        }
    }
}

pub trait PermissionWriter: Send + Sync + 'static {
    fn create_relation(
        &self,
        target: &RelationTarget,
        subject: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;

    fn delete_relation(
        &self,
        target: &RelationTarget,
        subject: &AuthAccountId,
    ) -> impl Future<Output = error_stack::Result<(), KernelError>> + Send;
}

pub trait DependOnPermissionWriter: Send + Sync {
    type PermissionWriter: PermissionWriter;
    fn permission_writer(&self) -> &Self::PermissionWriter;
}
