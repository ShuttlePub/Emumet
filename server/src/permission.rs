use std::collections::HashSet;
use std::hash::Hash;
use std::ops::Add;

struct Request<O>(HashSet<O>);

impl<O> Request<O> {
    pub fn new(objects: HashSet<O>) -> Self {
        Request(objects)
    }
}

pub trait ToRequest<T, O> {
    fn to_request(&self, target: &T) -> Request<O>;
}

#[derive(Debug, Clone)]
pub struct Permission<O>(Vec<HashSet<O>>);

impl<O: Eq + Hash + Clone> Permission<O> {
    pub fn new(perm: HashSet<O>) -> Self {
        Permission(vec![perm])
    }

    pub fn allows(&self, req: &Request<O>) -> bool {
        let request = &req.0;
        self.0.iter().all(|p| !(p & request).is_empty())
    }
}

impl<O> Add for Permission<O> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        let mut perm = self.0;
        perm.extend(rhs.0);
        Permission(perm)
    }
}

pub trait ToPermission<T, O> {
    fn to_permission(&self, target: T) -> Permission<O>;
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashMap;

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    enum Role {
        Admin,
        User,
        Guest,
    }

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    struct User {
        id: u64,
    }

    #[derive(Debug, Clone)]
    struct RoleManager(HashMap<u64, Role>);

    impl ToRequest<User, Role> for RoleManager {
        fn to_request(&self, target: &User) -> Request<Role> {
            Request(self.0.get(&target.id).map_or_else(
                HashSet::new,
                |role| vec![role.clone()].into_iter().collect::<HashSet<Role>>(),
            ))
        }
    }

    #[derive(Debug, Clone, Hash, PartialEq, Eq)]
    enum Action {
        Read,
        Write,
        Delete,
    }

    #[derive(Debug, Clone)]
    struct ActionRoles(HashMap<Action, HashSet<Role>>);

    impl ToPermission<Action, Role> for ActionRoles {
        fn to_permission(&self, target: Action) -> Permission<Role> {
            self.0.get(&target).map_or_else(
                || Permission::new(HashSet::new()),
                |roles| Permission::new(roles.clone()),
            )
        }
    }

    #[test]
    fn test() {
        let mut role_manager = RoleManager(HashMap::new());
        let mut action_roles = ActionRoles(HashMap::new());
        action_roles.0.insert(
            Action::Read,
            vec![Role::Admin, Role::User, Role::Guest]
                .into_iter()
                .collect::<HashSet<Role>>(),
        );
        action_roles.0.insert(
            Action::Write,
            vec![Role::Admin, Role::User]
                .into_iter()
                .collect::<HashSet<Role>>(),
        );
        action_roles.0.insert(
            Action::Delete,
            vec![Role::Admin].into_iter().collect::<HashSet<Role>>(),
        );

        let user = User { id: 1 };
        role_manager.0.insert(user.id, Role::User);

        let request = role_manager.to_request(&user);
        let read = action_roles.to_permission(Action::Read);
        let write = action_roles.to_permission(Action::Write);

        let new_perm = read + write;

        assert!(new_perm.allows(&request));
    }
}
