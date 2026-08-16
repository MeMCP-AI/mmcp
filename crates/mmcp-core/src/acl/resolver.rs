//! Effective role resolution over a membership set.

use crate::acl::EffectiveRole;
use crate::id::{OrgId, UserId};
use crate::identity::{Group, GroupOwner, Membership, Principal};

/// Compute the effective role for `user` on `group`, walking every
/// membership path in `memberships` and every org the user belongs to
/// in `user_orgs`.
///
/// Resolution semantics:
///
/// 1. If the group is user-owned and that user is the caller, the
///    owner implicitly holds [`Role::Admin`](crate::identity::Role::Admin).
/// 2. Direct `Membership { principal: Principal::User(user), .. }`
///    entries targeting `group` contribute their role.
/// 3. `Membership { principal: Principal::Org(org), .. }` entries
///    contribute their role if `org` is in `user_orgs`.
/// 4. The effective role is the maximum across all contributing
///    paths. An empty contributor set yields [`EffectiveRole::none`].
///
/// Group-of-groups propagation (`Principal::Group`) is not resolved
/// here because it requires recursive lookup; callers that use it
/// should expand nested groups into direct memberships before calling
/// the resolver, or this function should be extended to accept a
/// group-membership graph. Left for a later commit so the simple
/// case is covered and tested first.
#[must_use]
pub fn resolve_effective_role(
    user: UserId,
    user_orgs: &[OrgId],
    group: &Group,
    memberships: &[Membership],
) -> EffectiveRole {
    let mut effective = EffectiveRole::none();

    // Rule 1: user-owned group implies admin for the owner.
    if let GroupOwner::User(owner) = group.owner
        && owner == user
    {
        effective = effective.max(EffectiveRole::of(crate::identity::Role::Admin));
    }

    for m in memberships {
        if m.group != group.id {
            continue;
        }

        match m.principal {
            // Rule 2: direct user membership.
            Principal::User(u) if u == user => {
                effective = effective.max(EffectiveRole::of(m.role));
            }
            // Rule 3: org membership granting access to all org users.
            Principal::Org(o) if user_orgs.contains(&o) => {
                effective = effective.max(EffectiveRole::of(m.role));
            }
            // Group-of-groups propagation: deferred.
            Principal::Group(_) | Principal::User(_) | Principal::Org(_) => {}
        }
    }

    effective
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::id::{GroupId, OrgId, UserId};
    use crate::identity::{Group, GroupOwner, Membership, Principal, Role};
    use jiff::Timestamp;

    fn t() -> Timestamp {
        Timestamp::UNIX_EPOCH
    }

    fn make_group(owner: GroupOwner) -> Group {
        Group {
            id: GroupId::new(),
            slug: "test".to_string(),
            owner,
            display_name: None,
            created_at: t(),
        }
    }

    #[test]
    fn user_owned_group_grants_admin_to_owner() {
        let user = UserId::new();
        let group = make_group(GroupOwner::User(user));

        let role = resolve_effective_role(user, &[], &group, &[]);
        assert_eq!(role.role(), Some(Role::Admin));
    }

    #[test]
    fn non_owner_with_no_memberships_has_no_access() {
        let owner = UserId::new();
        let stranger = UserId::new();
        let group = make_group(GroupOwner::User(owner));

        let role = resolve_effective_role(stranger, &[], &group, &[]);
        assert_eq!(role.role(), None);
    }

    #[test]
    fn direct_user_membership_contributes_its_role() {
        let user = UserId::new();
        let group = make_group(GroupOwner::Org(OrgId::new()));
        let membership = Membership {
            group: group.id,
            principal: Principal::User(user),
            role: Role::Write,
            granted_at: t(),
        };

        let role = resolve_effective_role(user, &[], &group, &[membership]);
        assert_eq!(role.role(), Some(Role::Write));
    }

    #[test]
    fn org_membership_propagates_to_org_members() {
        let user = UserId::new();
        let org = OrgId::new();
        let group = make_group(GroupOwner::Org(org));
        let membership = Membership {
            group: group.id,
            principal: Principal::Org(org),
            role: Role::Read,
            granted_at: t(),
        };

        let role = resolve_effective_role(user, &[org], &group, &[membership]);
        assert_eq!(role.role(), Some(Role::Read));
    }

    #[test]
    fn effective_role_is_maximum_across_paths() {
        let user = UserId::new();
        let org = OrgId::new();
        let group = make_group(GroupOwner::Org(org));

        let memberships = vec![
            Membership {
                group: group.id,
                principal: Principal::User(user),
                role: Role::Read,
                granted_at: t(),
            },
            Membership {
                group: group.id,
                principal: Principal::Org(org),
                role: Role::Write,
                granted_at: t(),
            },
        ];

        let role = resolve_effective_role(user, &[org], &group, &memberships);
        assert_eq!(role.role(), Some(Role::Write));
    }

    #[test]
    fn memberships_for_other_groups_are_ignored() {
        let user = UserId::new();
        let group = make_group(GroupOwner::Org(OrgId::new()));
        let other_group = GroupId::new();

        let membership = Membership {
            group: other_group,
            principal: Principal::User(user),
            role: Role::Admin,
            granted_at: t(),
        };

        let role = resolve_effective_role(user, &[], &group, &[membership]);
        assert_eq!(role.role(), None);
    }

    #[test]
    fn allows_checks_include_weaker_requirements() {
        let user = UserId::new();
        let group = make_group(GroupOwner::Org(OrgId::new()));
        let membership = Membership {
            group: group.id,
            principal: Principal::User(user),
            role: Role::Write,
            granted_at: t(),
        };

        let effective = resolve_effective_role(user, &[], &group, &[membership]);
        assert!(effective.allows(Role::Read));
        assert!(effective.allows(Role::Write));
        assert!(!effective.allows(Role::Admin));
    }
}
