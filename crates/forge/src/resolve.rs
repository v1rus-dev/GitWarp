use crate::remote::RemoteRef;
use crate::{known_kind_for_host, Instance, ServiceKind};

/// The part of an account that remote resolution needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub id: String,
    pub instance: Instance,
    pub login: String,
}

/// Which account serves a remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution<'a> {
    /// Exactly one account (or the remembered one) applies.
    One(&'a AccountRef),
    /// Several accounts match; the user has to pick.
    Many(Vec<&'a AccountRef>),
    /// No account matches. `known_kind` is the service when the host is a
    /// public one, so the UI can offer "Add account for <host>".
    None { known_kind: Option<ServiceKind> },
}

/// [`resolve_with`] without a remembered choice.
pub fn resolve<'a>(accounts: &'a [AccountRef], remote: &RemoteRef) -> Resolution<'a> {
    resolve_with(accounts, remote, None)
}

/// Finds the accounts whose instance matches `remote`. A remembered account
/// id wins when that account still matches; a stale id is ignored.
pub fn resolve_with<'a>(
    accounts: &'a [AccountRef],
    remote: &RemoteRef,
    remembered_id: Option<&str>,
) -> Resolution<'a> {
    let mut matching: Vec<&AccountRef> = accounts
        .iter()
        .filter(|a| a.instance.matches(remote).is_some())
        .collect();
    if let Some(id) = remembered_id {
        if let Some(found) = matching.iter().find(|a| a.id == id) {
            return Resolution::One(found);
        }
    }
    match matching.len() {
        0 => Resolution::None {
            known_kind: known_kind_for_host(&remote.host),
        },
        1 => Resolution::One(matching.remove(0)),
        _ => Resolution::Many(matching),
    }
}
