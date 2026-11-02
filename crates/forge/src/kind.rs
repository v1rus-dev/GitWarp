/// The type of a hosting service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ServiceKind {
    GitHub,
    GitLab,
    /// Gitea and its fork Forgejo (Codeberg runs Forgejo).
    Gitea,
    Bitbucket,
    AzureDevOps,
}

impl ServiceKind {
    /// Every kind, in the order the "Add account" dialog lists them.
    pub const ALL: [ServiceKind; 5] = [
        ServiceKind::GitHub,
        ServiceKind::GitLab,
        ServiceKind::Gitea,
        ServiceKind::Bitbucket,
        ServiceKind::AzureDevOps,
    ];

    /// Human-readable name shown in the UI.
    pub fn display_name(self) -> &'static str {
        match self {
            ServiceKind::GitHub => "GitHub",
            ServiceKind::GitLab => "GitLab",
            ServiceKind::Gitea => "Gitea / Forgejo",
            ServiceKind::Bitbucket => "Bitbucket",
            ServiceKind::AzureDevOps => "Azure DevOps",
        }
    }

    /// Stable identifier for persisted settings; never change these values.
    pub fn id(self) -> &'static str {
        match self {
            ServiceKind::GitHub => "github",
            ServiceKind::GitLab => "gitlab",
            ServiceKind::Gitea => "gitea",
            ServiceKind::Bitbucket => "bitbucket",
            ServiceKind::AzureDevOps => "azure",
        }
    }

    /// Inverse of [`ServiceKind::id`].
    pub fn from_id(id: &str) -> Option<ServiceKind> {
        ServiceKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// What the service calls a code-review request: GitLab says "merge
    /// request", everyone else "pull request".
    pub fn pr_noun(self) -> &'static str {
        match self {
            ServiceKind::GitLab => "merge request",
            _ => "pull request",
        }
    }
}
