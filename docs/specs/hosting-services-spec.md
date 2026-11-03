# Hosting services (GitHub, GitLab, self-hosted, …) — specification

Status: approved for design, 2026-11-02. Implementation starts after the design pages are done.
Research behind it: `swarm-report/forge-integration/research.md`.

## 1. Goal

GitWarp talks to the services that host a repository's remotes: it signs in to them, uses that
sign-in for git over HTTPS and calls their APIs (repos, pull/merge requests, CI status).

Hard requirements from the user:

- **Self-hosted instances are first-class**: GitHub Enterprise Server, GitLab self-managed,
  Gitea/Forgejo, Azure DevOps Server — not only github.com / gitlab.com.
- **Any number of accounts, including several of the same type**: two GitHub accounts, gitlab.com
  plus `gitlab.company.com`, Codeberg, … at the same time.
- **Repositories with several remotes** (`origin`, `upstream`, `work`, …), and a remote with several
  push URLs (`git remote set-url --add --push`), are handled: every remote URL is resolved to its own
  service and account.

## 2. Terms

- **Service type**: GitHub, GitLab, Gitea/Forgejo, Bitbucket, Azure DevOps.
- **Instance**: a service type + base URL (`https://github.com`, `https://gitlab.company.com`).
  Known public instances: github.com, gitlab.com, codeberg.org (Forgejo), bitbucket.org, dev.azure.com.
- **Account**: instance + user login + credential. Several accounts may share an instance.
- **Credential**: a personal access token (any instance) or an OAuth token pair (where sign-in in the
  browser is available). Secrets are stored in the OS keychain (macOS Keychain, Windows Credential
  Manager, Secret Service on Linux); `settings.json` keeps only `{id, type, base URL, login, label,
  method}`.

## 3. Functional requirements

### 3.1 Accounts (Settings → Accounts, new page)

- AC-1 The page lists all accounts: service icon, label (default = login), instance host, sign-in
  method, status. Empty state explains what accounts are for and offers "Add account".
- AC-2 Status per account: connected · checking · token invalid or revoked (Reconnect) · token
  expires in ≤ 7 days (warning) · instance unreachable (Retry). Status is re-checked on page open
  and before the account is used.
- AC-3 Actions per account: Reconnect (new token / sign in again), Rename label, Remove (confirm;
  removes the secret from the keychain), Open profile on the instance.
- AC-4 "Hosting accounts" is removed from Settings → Identity; Identity shows one line linking to
  Accounts.
- AC-5 Import: when `gh` or `glab` on this machine are signed in to an instance with no GitWarp
  account, the page offers to import those logins (one row per login, Import / Dismiss).
  Feasibility of reading their tokens is a spike; the design must work if import is unavailable.

### 3.2 Add account (dialog)

- AC-6 Step 1 — service type: GitHub, GitLab, Gitea/Forgejo, Bitbucket, Azure DevOps.
- AC-7 Step 2 — instance: the public instance is preselected ("github.com"); "Self-hosted" reveals a
  URL field. GitWarp probes the URL and shows: checking · detected (type + version) · the URL is a
  different service type (offer to switch) · unreachable · TLS certificate not trusted (show the
  certificate fingerprint, let the user trust it for this instance).
- AC-8 Step 3 — sign-in method:
  - **Token** — always available. Token field (masked, show/hide), a "Create a token" button that
    opens the instance's token page in the browser, and the list of permissions the token needs
    (GitHub classic: `repo`, `read:user`, `workflow`; GitLab: `api`, `read_repository`,
    `write_repository`; Gitea: repository + user read/write; Bitbucket API token:
    `read:repository:bitbucket`, `write:repository:bitbucket`, pull-request read/write; Azure: Code
    read & write).
  - **Sign in with browser** — available by default for github.com, gitlab.com, codeberg.org. For a
    self-hosted instance it is available after the user enters the client ID of an OAuth application
    registered on that instance (help text says where to register it and which redirect URI /
    scopes to use). Not offered for Bitbucket and Azure DevOps in v1.
  - Device-code variant (GitHub, GitLab): the dialog shows a short code with Copy, "Open browser",
    the URL to type by hand, a countdown, and waits. Browser-redirect variant (Gitea/Forgejo,
    GitLab fallback): the dialog says the browser was opened and waits, with "Open again" and Cancel.
- AC-9 Result states: success (avatar, login, "Connected as …"), wrong token, missing permissions
  (list which), denied in browser, code expired, network error. On success the account appears in
  the list; adding a second account to the same instance asks for a label.
- AC-10 The same flow is used for Reconnect (prefilled type/instance, step 3 only).

### 3.3 Remotes ↔ accounts

- AC-11 Every remote URL (fetch URL and every push URL; https, ssh and scp-like forms) is parsed to
  instance + owner + repo. The sidebar remote row shows the service icon and the account in use.
- AC-12 Account choice for a remote: if exactly one account matches the instance, it is used;
  if several match, the user picks one once per repository+remote (remembered), changeable from
  the remote's context menu "Account…". If none matches and the instance is a known service,
  the remote row offers "Add account for <host>".
- AC-13 Git over HTTPS uses the chosen account's token automatically through the askpass helper;
  the existing credential prompt remains for hosts without an account.
- AC-14 A remote with several push URLs shows each target host; push reports per-target result.

### 3.4 Features using the APIs (ordered; each is a separate implementation stage)

- AC-15 Open on host: commit, branch, tag, file (at the selected commit), and pull/merge request
  open in the browser. Needs only URL parsing, works without an account.
- AC-16 Clone dialog gets a "From account" tab: account switcher, search, list of the account's
  repositories (name, owner, private/fork badges, updated), select → fills the URL and folder.
- AC-17 Pull/merge request badge on branches (sidebar and history ref labels): number, state
  (open/draft/merged/closed), review state; click opens a PR popover with Open on host / Checkout.
- AC-18 CI status on commits: dot in history rows (pending/running, passed, failed, cancelled) and a
  list of checks/pipelines in commit details with links.
- AC-19 Create pull/merge request from a branch: target remote + branch, title/description prefilled
  from commits, draft toggle, reviewers optional; pushes the branch first if it is not on the remote.
- AC-20 Pull/merge requests panel in the sidebar: list for the current repository (mine / review
  requested / all), checkout a PR into a local branch.

## 4. Non-functional

- Secrets never in `settings.json`, logs or error text.
- API calls run off the UI thread; failures show inline states, never block the window.
- Rate limits respected (GitHub 5 000 req/h per user): conditional requests and caching of PR/CI
  data per repository; background refresh only for the open repository.
- Works offline: everything that does not need the network keeps working; account rows show
  "unreachable" instead of errors.

## 5. Out of scope for v1

Issues, code review comments inside GitWarp, notifications, creating repositories on a service,
OAuth for Bitbucket and Azure DevOps.

## 6. Design deliverables (OpenDesign, project «gitlane Git Client Design»)

1. `settings-accounts.html` — Settings → Accounts with all states of AC-1…AC-5; Identity page updated (AC-4).
2. `dialogs-account.html` — Add / Reconnect account dialog, every step and state of AC-6…AC-10.
3. `remotes-accounts.html` — sidebar remotes with service + account, account picker, "Add account
   for host", multi-push-URL remote (AC-11…AC-14).
4. `hosting-features.html` — Open on host menus, Clone "From account" tab, PR badges and popover,
   CI status in history and details, Create PR dialog, PR panel (AC-15…AC-20).

## 7. Open technical items (spikes before implementation planning)

Reading `gh`/`glab` tokens for import; keychain on Linux without a running Secret Service; OAuth apps
registered for GitWarp on github.com (device flow enabled), gitlab.com and codeberg.org — owner
account to be decided; GHES device-flow support.
