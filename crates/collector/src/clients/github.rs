// Copyright Alexandre D. Díaz
use crate::gitclient::{
    extract_migration_module_name, parse_created_at, GitClient, PullRequestInfo, RepoInfo,
};

const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_BASE_URL: &str = "https://api.github.com/";
const GITHUB_LIMIT_PER_PAGE: usize = 50;
const GITHUB_LIMIT_PAGES: usize = 255;

#[derive(Debug)]
pub struct GithubClient {
    token: String,
    client: reqwest::Client,
}

impl GitClient for GithubClient {
    fn new(token: &str, _base_url: &str) -> Self {
        let client_result = reqwest::Client::builder().build();
        let client = match client_result {
            Ok(cl) => cl,
            Err(e) => panic!("Problem creating the client: {e:?}"),
        };
        Self {
            token: token.into(),
            client,
        }
    }

    async fn request(&self, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        let full_url = format!("{GITHUB_BASE_URL}{url}");
        let res = self
            .client
            .get(full_url)
            .header(reqwest::header::USER_AGENT, "OGHCollector")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .send()
            .await?;
        Ok(res)
    }

    async fn request_json(&self, url: &str) -> Result<serde_json::Value, reqwest::Error> {
        let req = self.request(url).await?;
        req.json().await
    }

    async fn get_org_repos(
        &self,
        org_name: &str,
        per_page: &usize,
        page: &usize,
    ) -> Result<serde_json::Value, reqwest::Error> {
        let res = self
            .request_json(
                format!("orgs/{org_name}/repos?sort=updated&per_page={per_page}&page={page}")
                    .as_str(),
            )
            .await?;
        Ok(res)
    }

    async fn clone_org_repos(&self, org_name: &str, branch: &str, dest: &str) -> Vec<RepoInfo> {
        let mut page_count: usize = 1;
        let mut repos: Vec<RepoInfo> = Vec::new();
        while page_count < GITHUB_LIMIT_PAGES {
            let org_repos = match self
                .get_org_repos(org_name, &GITHUB_LIMIT_PER_PAGE, &page_count)
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    log::error!("Can't fetch repos of '{org_name}' (page {page_count}): {err}");
                    break;
                }
            };
            // Errors (rate limit, bad token, ...) come back as an object with
            // a "message" field instead of the repo array.
            let org_repos_items = match org_repos.as_array() {
                Some(arr) => arr,
                _ => {
                    log::error!(
                        "Unexpected GitHub response for '{org_name}': {}",
                        org_repos["message"].as_str().unwrap_or("unknown error")
                    );
                    break;
                }
            };
            if org_repos_items.is_empty() {
                break;
            }
            for repo_info in org_repos_items.iter() {
                let repo_owner_login = repo_info["owner"]["login"].as_str().unwrap_or("");
                let repo_name = repo_info["name"].as_str().unwrap_or("");
                let repo_url = repo_info["clone_url"].as_str().unwrap_or("");
                if repo_owner_login.is_empty() || repo_name.is_empty() || repo_url.is_empty() {
                    continue;
                }
                let repo_info_opt =
                    self.clone_or_update_repo(repo_owner_login, repo_name, repo_url, branch, dest);
                match repo_info_opt {
                    Some(info) => repos.push(info),
                    _ => log::info!("'{repo_url}' Is not a valid Odoo modules repository!"),
                }
            }
            if org_repos_items.len() < GITHUB_LIMIT_PER_PAGE {
                break;
            }
            page_count += 1;
        }
        repos
    }

    async fn get_repo_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
        per_page: &usize,
        page: &usize,
    ) -> Result<serde_json::Value, reqwest::Error> {
        self.request_json(
            format!(
                "repos/{full_path}/pulls?state=open&base={branch}&per_page={per_page}&page={page}"
            )
            .as_str(),
        )
        .await
    }

    async fn get_open_migration_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
    ) -> Vec<PullRequestInfo> {
        let mut page_count: usize = 1;
        let mut prs: Vec<PullRequestInfo> = Vec::new();
        while page_count < GITHUB_LIMIT_PAGES {
            let pulls = match self
                .get_repo_pull_requests(full_path, branch, &GITHUB_LIMIT_PER_PAGE, &page_count)
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    log::error!("Can't fetch pull requests of '{full_path}': {err}");
                    break;
                }
            };
            let pull_items = match pulls.as_array() {
                Some(arr) => arr,
                _ => break,
            };
            if pull_items.is_empty() {
                break;
            }
            for pull in pull_items {
                let head_ref = pull["head"]["ref"].as_str().unwrap_or("");
                if let Some(module_technical_name) = extract_migration_module_name(head_ref) {
                    let created_at = pull["created_at"].as_str().and_then(parse_created_at);
                    let head_sha = pull["head"]["sha"].as_str().unwrap_or("");
                    let ci_status = self.get_commit_ci_status(full_path, head_sha).await;
                    prs.push(PullRequestInfo {
                        number: pull["number"].as_i64().unwrap_or(0),
                        title: pull["title"].as_str().unwrap_or("").to_string(),
                        module_technical_name,
                        created_at,
                        ci_status,
                    });
                }
            }
            if pull_items.len() < GITHUB_LIMIT_PER_PAGE {
                break;
            }
            page_count += 1;
        }
        prs
    }

    async fn is_pull_request_merged(&self, full_path: &str, number: &i64) -> Option<bool> {
        let pull = self
            .request_json(&format!("repos/{full_path}/pulls/{number}"))
            .await
            .ok()?;
        pull["merged"].as_bool()
    }
}

impl GithubClient {
    /// Combines the classic combined-status API (used by e.g. OCA's
    /// runboat build) with the checks API (GitHub Actions runs, e.g. tests
    /// and pre-commit): on real OCA repos neither one alone reliably
    /// reflects the PR's actual CI state - some PRs only report through
    /// one of the two, so a check missing from one is looked up in the
    /// other before treating the PR as having no CI at all.
    async fn get_commit_ci_status(&self, full_path: &str, sha: &str) -> Option<String> {
        if sha.is_empty() {
            return None;
        }

        let status = self
            .request_json(&format!("repos/{full_path}/commits/{sha}/status"))
            .await
            .ok();
        let status_state = status.as_ref().and_then(|s| {
            if s["total_count"].as_i64().unwrap_or(0) == 0 {
                None
            } else {
                s["state"].as_str().map(str::to_string)
            }
        });

        let check_runs = self
            .request_json(&format!("repos/{full_path}/commits/{sha}/check-runs"))
            .await
            .ok();
        let check_runs_state = check_runs.as_ref().and_then(|c| {
            let runs = c["check_runs"].as_array()?;
            if runs.is_empty() {
                return None;
            }
            if runs
                .iter()
                .any(|r| r["status"].as_str() != Some("completed"))
            {
                return Some("pending".to_string());
            }
            if runs.iter().any(|r| {
                !matches!(
                    r["conclusion"].as_str(),
                    Some("success") | Some("neutral") | Some("skipped")
                )
            }) {
                return Some("failure".to_string());
            }
            Some("success".to_string())
        });

        match (status_state.as_deref(), check_runs_state.as_deref()) {
            (Some("failure") | Some("error"), _) | (_, Some("failure")) => {
                Some("failure".to_string())
            }
            (Some("pending"), _) | (_, Some("pending")) => Some("pending".to_string()),
            (Some("success"), _) | (_, Some("success")) => Some("success".to_string()),
            _ => None,
        }
    }
}
