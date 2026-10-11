// Copyright Alexandre D. Díaz
use duct::cmd;
use regex::Regex;
use std::fs;
use std::path::Path;

pub struct RepoInfo {
    pub name: String,
    pub org: String,
    pub clone_path: String,
    pub full_path: String,
}

impl RepoInfo {
    pub fn get_name(&self) -> &str {
        &self.name
    }
    pub fn get_org(&self) -> &str {
        &self.org
    }
    pub fn get_clone_path(&self) -> &str {
        &self.clone_path
    }
    pub fn get_full_path(&self) -> &str {
        &self.full_path
    }
}

#[derive(Debug, Clone)]
pub struct PullRequestInfo {
    pub number: i64,
    pub title: String,
    pub module_technical_name: String,
    /// PR/MR creation date, in sqlite text format (`%Y-%m-%d %H:%M:%S`),
    /// `None` if the provider's `created_at` was missing/unparseable.
    pub created_at: Option<String>,
    /// Last-activity date, in the same format. Providers only expose this for
    /// free as the PR/MR's own `updated_at` (bumped by commits, comments,
    /// label changes, etc.) in the same list response already fetched here -
    /// a real "last comment" date would need one extra API call per PR, so
    /// this is used as the closest available proxy for "last message".
    pub last_message_at: Option<String>,
    /// Normalized CI signal: `Some("success")`, `Some("pending")`,
    /// `Some("failure")`, or `None` when no CI is configured for the head
    /// commit / the provider reported nothing usable.
    pub ci_status: Option<String>,
}

/// Parses a provider's RFC3339 `created_at` into the sqlite text format
/// used to store dates in this project (see `sqlitedb::utils::date`).
pub fn parse_created_at(raw: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| sqlitedb::utils::date::to_sqlite_datetime(dt.with_timezone(&chrono::Utc)))
}

/// OCA migration convention: head branch `{version}-mig-{module_technical_name}`,
/// optionally followed by a `-{discard}` suffix (e.g. `16.0-mig-sale_commission-wip`).
/// Returns the module technical name if it matches.
///
/// Odoo technical names never contain `-`, only `_`. So if the captured tail
/// already has an `_` in it, it's a real name with one discard segment tacked
/// on the end - drop everything after the last `-`. If it has no `_` at all,
/// the whole thing was typed with `-` instead of `_` and there's no way to
/// tell a discard suffix apart from the name - just normalize every `-` to `_`.
/// ponytail: ambiguous when both a hyphenated name AND a discard suffix are
/// present (e.g. `sale-order-line-wip`) - the discard ends up folded into the
/// name. Add a real module-name lookup if this proves common in practice.
pub fn extract_migration_module_name(head_ref: &str) -> Option<String> {
    let re = Regex::new(r"^[0-9]+\.[0-9]+-mig-(.+)$").unwrap();
    re.captures(head_ref).and_then(|caps| caps.get(1)).map(|m| {
        let raw = m.as_str();
        if raw.contains('_') {
            raw.rsplit_once('-')
                .map_or(raw, |(name, _discard)| name)
                .to_string()
        } else {
            raw.replace('-', "_")
        }
    })
}

/// Runs git in `dir`, returning trimmed stdout, or stdout+stderr as the error.
/// `safe.directory=*` avoids "dubious ownership" aborts on volumes owned by
/// another uid; `GIT_TERMINAL_PROMPT=0` makes auth failures fail instead of hang.
fn git_run<S: AsRef<str>>(dir: &str, args: &[S]) -> Result<String, String> {
    git_run_env(dir, args, &[])
}

/// `git_run` with extra environment variables (see `git_auth_env`).
fn git_run_env<S: AsRef<str>>(
    dir: &str,
    args: &[S],
    env: &[(String, String)],
) -> Result<String, String> {
    let args: Vec<&str> = ["-c", "safe.directory=*"]
        .into_iter()
        .chain(args.iter().map(|a| a.as_ref()))
        .collect();
    let mut expr = cmd("git", args).dir(dir).env("GIT_TERMINAL_PROMPT", "0");
    for (key, value) in env {
        expr = expr.env(key, value);
    }
    let out = expr
        .stdin_null()
        .stderr_to_stdout()
        .stdout_capture()
        .unchecked()
        .run()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if out.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

/// Git credentials for `clone`/`fetch` (private repositories): a credential
/// helper scoped to the repository's host, handed to git through `GIT_CONFIG_*`
/// env vars, that answers with the token from its own env var. Git only asks
/// after a 401, so public repositories are still fetched anonymously (a token
/// without git scopes can't break them), and the token is never written to the
/// clone's `.git/config` (on the shared data volume) nor shown by `ps`. The
/// empty helper first drops any helper inherited from the user's git config.
fn git_auth_env(repo_url: &str, user: &str, token: &str) -> Vec<(String, String)> {
    let Ok(url) = reqwest::Url::parse(repo_url) else {
        return Vec::new();
    };
    let Some(host) = url.host_str().filter(|_| !token.is_empty()) else {
        return Vec::new();
    };
    let port = url.port().map_or(String::new(), |p| format!(":{p}"));
    let key = format!("credential.{}://{host}{port}.helper", url.scheme());
    let helper = format!(
        r#"!f() {{ test "$1" = get || return 0; printf 'username={user}\npassword=%s\n' "${GIT_TOKEN_ENV}"; }}; f"#
    );
    [
        ("GIT_CONFIG_COUNT", "2".to_string()),
        ("GIT_CONFIG_KEY_0", key.clone()),
        ("GIT_CONFIG_VALUE_0", String::new()),
        ("GIT_CONFIG_KEY_1", key),
        ("GIT_CONFIG_VALUE_1", helper),
        (GIT_TOKEN_ENV, token.to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect()
}

const GIT_TOKEN_ENV: &str = "OGHCOLLECTOR_GIT_TOKEN";

pub trait GitClient {
    fn new(token: &str, base_url: &str) -> Self;

    /// `(user, token)` git authenticates with over HTTPS: the token is the
    /// API one, the user whatever the provider expects alongside a token.
    fn git_credentials(&self) -> (&str, &str);

    fn clone_or_update_repo(
        &self,
        org_name: &str,
        repo_name: &str,
        repo_url: &str,
        branch: &str,
        dest: &str,
    ) -> Option<RepoInfo> {
        let clone_path = format!("{dest}/{org_name}/{repo_name}");
        let base_dir = format!("{dest}/{org_name}");
        let origin_branch = format!("origin/{branch}");
        let (git_user, git_token) = self.git_credentials();
        let auth_env = git_auth_env(repo_url, git_user, git_token);
        // `.git` check (not just `exists()`): git run inside a non-repo dir
        // walks up to any parent repo and would act on that one instead.
        let mut needs_clone = !Path::new(&clone_path).join(".git").exists();
        if !needs_clone {
            log::info!("Updating repo: {repo_name} @ {branch}");
            // Never delete the clone because of a fetch failure (network down,
            // rate limit, auth) or a missing upstream branch: neither means the
            // checkout is broken, and the clone is shared by every Odoo version.
            // Explicit refspec: clones made single-branch (older versions of
            // this code) only track their first branch, so a plain
            // `fetch origin` never brings in the requested one.
            let refspec = format!("+refs/heads/{branch}:refs/remotes/{origin_branch}");
            let fetched = git_run_env(
                &clone_path,
                &["fetch", "--prune", "origin", &refspec],
                &auth_env,
            );
            if let Err(err) = fetched {
                if err.contains("couldn't find remote ref") {
                    log::info!(
                        "'{repo_name}': branch '{branch}' doesn't exist upstream - skipping"
                    );
                } else {
                    log::error!("'{repo_name}': fetch failed - keeping the clone, skipping: {err}");
                }
                return None;
            }
            let updated = [
                vec!["reset", "--hard", &origin_branch],
                vec!["clean", "-fdx"],
                vec!["switch", "-C", branch, &origin_branch],
            ]
            .iter()
            .all(|args| match git_run(&clone_path, args) {
                Ok(_) => true,
                Err(err) => {
                    log::warn!("'{repo_name}': git {} failed: {err}", args[0]);
                    false
                }
            });
            if updated {
                log::info!("Repo updated & cleaned: {repo_name} @ {branch}");
            } else {
                log::warn!("Update of '{repo_name}' failed - re-cloning");
                needs_clone = true;
            }
        }
        if needs_clone {
            log::info!("Cloning repo: {repo_name} @ {branch}");
            if let Err(err) = fs::create_dir_all(&base_dir) {
                log::error!("Cannot create directory '{base_dir}': {err}");
                return None;
            }
            // Clone aside and swap only on success, so a failed (re)clone never
            // leaves the repo missing.
            let tmp_name = format!("{repo_name}.tmp");
            let tmp_path = format!("{base_dir}/{tmp_name}");
            let _ = fs::remove_dir_all(&tmp_path);
            let cloned = git_run_env(
                &base_dir,
                &[
                    "clone",
                    "--no-single-branch",
                    "--branch",
                    branch,
                    repo_url,
                    &tmp_name,
                ],
                &auth_env,
            );
            if let Err(err) = cloned {
                if err.contains("not found in upstream") {
                    log::info!(
                        "'{repo_name}': branch '{branch}' doesn't exist upstream - skipping"
                    );
                } else {
                    log::error!("'{repo_name}': clone @ {branch} failed: {err}");
                }
                let _ = fs::remove_dir_all(&tmp_path);
                return None;
            }
            let swapped = (!Path::new(&clone_path).exists()
                || fs::remove_dir_all(&clone_path).is_ok())
                && fs::rename(&tmp_path, &clone_path).is_ok();
            if !swapped {
                log::error!("'{repo_name}': cannot move fresh clone into '{clone_path}'");
                let _ = fs::remove_dir_all(&tmp_path);
                return None;
            }
        }

        // HEAD must be exactly the tip of the requested branch on the requested
        // branch: a stale/other-branch tree would be analyzed and stored under
        // this version's label.
        let rev = |r: &str| git_run(&clone_path, &["rev-parse", r]);
        let (head, tip) = match (rev("HEAD"), rev(&origin_branch)) {
            (Ok(h), Ok(t)) => (h, t),
            (h, t) => {
                log::error!(
                    "'{repo_name}': rev-parse failed: {:?} {:?}",
                    h.err(),
                    t.err()
                );
                return None;
            }
        };
        if head != tip {
            log::error!("'{repo_name}': HEAD is not at '{origin_branch}' - skipping");
            return None;
        }
        match git_run(&clone_path, &["symbolic-ref", "--short", "-q", "HEAD"]) {
            Ok(b) if b == branch => {}
            other => {
                log::error!(
                    "'{repo_name}': checked out {other:?}, requested '{branch}' - skipping"
                );
                return None;
            }
        }

        Some(RepoInfo {
            name: repo_name.into(),
            org: org_name.into(),
            clone_path,
            full_path: format!("{org_name}/{repo_name}"),
        })
    }

    async fn request(&self, url: &str) -> Result<reqwest::Response, reqwest::Error>;

    async fn request_json(&self, url: &str) -> Result<serde_json::Value, reqwest::Error> {
        let req = self.request(url).await?;
        req.json().await
    }

    async fn get_org_repos(
        &self,
        org_name: &str,
        per_page: &usize,
        page: &usize,
    ) -> Result<serde_json::Value, reqwest::Error>;

    async fn clone_org_repos(&self, org_name: &str, branch: &str, dest: &str) -> Vec<RepoInfo>;

    /// Clones/updates what a collector source names: one repository
    /// (`owner/repo`) or every repository of an organization/group.
    async fn clone_source(&self, source: &str, branch: &str, dest: &str) -> Vec<RepoInfo>;

    async fn get_repo_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
        per_page: &usize,
        page: &usize,
    ) -> Result<serde_json::Value, reqwest::Error>;

    async fn get_open_migration_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
    ) -> Vec<PullRequestInfo>;

    /// Whether a PR/MR that's no longer open was actually merged - one extra
    /// GET per newly-closed PR, called right before `delete_outdated` so the
    /// "avg. time open before close" stat only counts merged PRs, not rejections.
    /// `None` on a lookup failure (network/rate-limit/404): treated as "not merged"
    /// by the caller, so a transient error undercounts rather than pollutes.
    async fn is_pull_request_merged(&self, full_path: &str, number: &i64) -> Option<bool>;
}

#[cfg(test)]
mod tests {
    use super::{extract_migration_module_name, git_auth_env};

    #[test]
    fn test_git_auth_env_scopes_the_helper_to_the_repo_host() {
        let env = git_auth_env(
            "https://git.acme.dev:8443/grp/sub/repo.git",
            "oauth2",
            "tok",
        );
        let get = |k: &str| {
            env.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        let key = Some("credential.https://git.acme.dev:8443.helper");
        assert_eq!(get("GIT_CONFIG_COUNT"), Some("2"));
        assert_eq!(
            (get("GIT_CONFIG_KEY_0"), get("GIT_CONFIG_VALUE_0")),
            (key, Some(""))
        );
        assert_eq!(get("GIT_CONFIG_KEY_1"), key);
        assert!(get("GIT_CONFIG_VALUE_1").is_some_and(|h| !h.contains("tok")));
        assert_eq!(get("OGHCOLLECTOR_GIT_TOKEN"), Some("tok"));
        assert!(git_auth_env("https://github.com/OCA/web.git", "x", "").is_empty());
    }

    #[test]
    fn test_extract_migration_module_name_matches_oca_convention() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-sale_commission"),
            Some("sale_commission".to_string())
        );
        assert_eq!(
            extract_migration_module_name("17.0-mig-account_analytic_tag_taxonomies"),
            Some("account_analytic_tag_taxonomies".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_rejects_non_migration_branches() {
        assert_eq!(extract_migration_module_name("16.0-fix-foo"), None);
        assert_eq!(extract_migration_module_name("master"), None);
        assert_eq!(extract_migration_module_name("16.0-mig-"), None);
        assert_eq!(extract_migration_module_name("mig-sale_commission"), None);
    }

    #[test]
    fn test_extract_migration_module_name_drops_discard_suffix() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre_modulo_algo-BAD"),
            Some("nombre_modulo_algo".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_normalizes_hyphenated_name() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre-modulo-algo"),
            Some("nombre_modulo_algo".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_ambiguous_hyphenated_with_discard() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre-modulo-algo-BAD"),
            Some("nombre_modulo_algo_BAD".to_string())
        );
    }
}
