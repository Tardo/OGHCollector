// Copyright Alexandre D. Díaz
use duct::cmd;
use reqwest::StatusCode;
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct RepoInfo {
    pub name: String,
    pub org: String,
    pub clone_path: String,
    pub full_path: String,
    /// Clone URL (GitHub or a GitLab host), stored so links point back to it.
    pub url: String,
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
    pub fn get_url(&self) -> &str {
        &self.url
    }
}

#[derive(Debug, Clone)]
pub struct PullRequestInfo {
    pub number: i64,
    pub title: String,
    /// Source branch (OCA migration convention, see
    /// `pull_requests::extract_migration_module_name`).
    pub head_ref: String,
    pub head_sha: String,
    /// Description (what `pr_edit` templates start from).
    pub description: String,
    pub author: Option<String>,
    pub is_draft: bool,
    pub labels: Vec<String>,
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
    /// `Some("failure")`, or `None` when the listing doesn't carry it
    /// (GitHub: see `GitClient::get_ci_status`) or nothing usable came back.
    pub ci_status: Option<String>,
}

/// Parses a provider's RFC3339 `created_at` into the sqlite text format
/// used to store dates in this project (see `sqlitedb::utils::date`).
pub fn parse_created_at(raw: &str) -> Option<String> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| sqlitedb::utils::date::to_sqlite_datetime(dt.with_timezone(&chrono::Utc)))
}

/// Runs git in `dir`, returning trimmed stdout, or stdout+stderr as the error.
/// `safe.directory=*` avoids "dubious ownership" aborts on volumes owned by
/// another uid; `GIT_TERMINAL_PROMPT=0` makes auth failures fail instead of hang,
/// and the low-speed limit a stalled clone/fetch (git has no timeout of its
/// own); 5 minutes, so a slow but moving `odoo/odoo` clone isn't cut.
pub(crate) fn git_run<S: AsRef<str>>(dir: &str, args: &[S]) -> Result<String, String> {
    git_run_env(dir, args, &[])
}

/// `git_run` with extra environment variables (see `git_auth_env`).
pub(crate) fn git_run_env<S: AsRef<str>>(
    dir: &str,
    args: &[S],
    env: &[(String, String)],
) -> Result<String, String> {
    let args: Vec<&str> = [
        "-c",
        "safe.directory=*",
        "-c",
        "http.lowSpeedLimit=1000",
        "-c",
        "http.lowSpeedTime=300",
    ]
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
pub(crate) fn git_auth_env(repo_url: &str, user: &str, token: &str) -> Vec<(String, String)> {
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

/// The collector's HTTP client. reqwest never times out by default: one
/// stalled connection would hang the job, and with it every later one (the
/// `--listen` daemon runs them one at a time). A timeout is a
/// network error, retried by `request_json`.
pub fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap_or_else(|e| panic!("Problem creating the client: {e:?}"))
}

const API_ATTEMPTS: u32 = 3;

/// How long to wait before retrying an API answer, `None` = don't. Rate
/// limits (GitHub: 403/429 with `x-ratelimit-remaining: 0`, GitLab: 429; both
/// may send `retry-after`) wait for their reset, capped at GitHub's 1h window.
/// A 403 without those headers is a real "no access".
fn retry_delay(res: &reqwest::Response) -> Option<Duration> {
    let status = res.status();
    if status.is_server_error() {
        return Some(Duration::from_secs(10));
    }
    if status != StatusCode::TOO_MANY_REQUESTS && status != StatusCode::FORBIDDEN {
        return None;
    }
    let header =
        |name: &str| -> Option<u64> { res.headers().get(name)?.to_str().ok()?.parse().ok() };
    let secs = if let Some(secs) = header("retry-after") {
        secs
    } else if let (Some(0), Some(reset)) = (
        header("x-ratelimit-remaining").or_else(|| header("ratelimit-remaining")),
        header("x-ratelimit-reset").or_else(|| header("ratelimit-reset")),
    ) {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        reset.saturating_sub(now) + 1
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        60
    } else {
        return None;
    };
    Some(Duration::from_secs(secs.min(3600)))
}

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
            url: repo_url.into(),
        })
    }

    async fn request(&self, url: &str) -> Result<reqwest::Response, reqwest::Error>;

    /// A non-2xx answer is an `Err` (it used to parse as an "error object"
    /// JSON that callers read as "no repos"/"no PRs", so a bad token or a
    /// rate limit collected nothing without failing). Rate limits, server
    /// errors and network errors are retried first (see `retry_delay`).
    async fn request_json(&self, url: &str) -> Result<serde_json::Value, reqwest::Error> {
        let mut attempt = 1;
        loop {
            let res = self.request(url).await;
            let (wait, what) = match &res {
                Ok(r) => (retry_delay(r), r.status().to_string()),
                Err(err) if err.is_timeout() => (Some(Duration::from_secs(10)), "timed out".into()),
                Err(err) => (Some(Duration::from_secs(10)), err.to_string()),
            };
            match wait {
                Some(wait) if attempt < API_ATTEMPTS => {
                    log::warn!(
                        "API request '{url}' failed ({what}), retrying in {}s...",
                        wait.as_secs()
                    );
                    tokio::time::sleep(wait).await;
                    attempt += 1;
                }
                _ => return res?.error_for_status()?.json().await,
            }
        }
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

    /// Every open PR/MR against `branch`. `None` when (part of) the listing
    /// failed or hit the page cap: unknown, not "none open", so the caller
    /// must not close the PRs it has stored.
    async fn get_open_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
    ) -> Option<Vec<PullRequestInfo>>;

    /// CI of a commit, for providers whose PR listing doesn't carry it
    /// (requests per call, so only asked for heads that changed or were
    /// still pending).
    async fn get_ci_status(&self, _full_path: &str, _sha: &str) -> Option<String> {
        None
    }

    /// Fetches a PR/MR head into the clone: objects only, no ref is kept (the
    /// caller knows the head sha from the listing), so the shared checkout and
    /// its branches are never touched.
    fn fetch_pull_request(
        &self,
        clone_path: &str,
        repo_url: &str,
        number: i64,
    ) -> Result<(), String> {
        let (git_user, git_token) = self.git_credentials();
        let refspec = oghutils::git::pull_request_ref(repo_url, number);
        git_run_env(
            clone_path,
            &["fetch", "--no-tags", "origin", &refspec],
            &git_auth_env(repo_url, git_user, git_token),
        )
        .map(|_| ())
    }

    /// Whether a PR/MR that's no longer open was actually merged - one extra
    /// GET per newly-closed PR, called right before `delete_outdated` so the
    /// "avg. time open before close" stat only counts merged PRs, not rejections.
    /// `None` on a lookup failure (network/rate-limit/404): treated as "not merged"
    /// by the caller, so a transient error undercounts rather than pollutes.
    async fn is_pull_request_merged(&self, full_path: &str, number: &i64) -> Option<bool>;
}

#[cfg(test)]
mod tests {
    use super::git_auth_env;

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
}
