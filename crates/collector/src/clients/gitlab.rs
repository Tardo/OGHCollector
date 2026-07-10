// Copyright Alexandre D. Díaz
// Adapted for GitLab
use crate::gitclient::{
    extract_migration_module_name, parse_created_at, GitClient, PullRequestInfo, RepoInfo,
};

const GITLAB_BASE_URL: &str = "https://gitlab.com/api/v4/";
const GITLAB_LIMIT_PER_PAGE: usize = 100; // GitLab permite hasta 100
const GITLAB_LIMIT_PAGES: usize = 255;

#[derive(Debug)]
pub struct GitlabClient {
    token: String,
    base_url: String,
    client: reqwest::Client,
}

impl GitClient for GitlabClient {
    fn new(token: &str, base_url: &str) -> Self {
        let client_result = reqwest::Client::builder().build();
        let client = match client_result {
            Ok(cl) => cl,
            Err(e) => panic!("Problem creating the client: {e:?}"),
        };
        let base_url_san = if base_url.is_empty() {
            GITLAB_BASE_URL
        } else {
            base_url
        };
        Self {
            token: token.into(),
            base_url: base_url_san.into(),
            client,
        }
    }

    async fn request(&self, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        let full_url = format!("{}{url}", self.base_url);
        let res = self
            .client
            .get(full_url)
            .header(reqwest::header::USER_AGENT, "OGHCollector") // puedes cambiarlo si quieres
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            )
            .header(reqwest::header::ACCEPT, "application/json")
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
                format!(
                    "groups/{}/projects?sort=updated_desc&per_page={}&page={}&simple=true",
                    urlencoding::encode(org_name),
                    per_page,
                    page
                )
                .as_str(),
            )
            .await?;
        Ok(res)
    }

    async fn clone_org_repos(&self, org_name: &str, branch: &str, dest: &str) -> Vec<RepoInfo> {
        let mut page_count: usize = 1;
        let mut repos: Vec<RepoInfo> = Vec::new();

        while page_count < GITLAB_LIMIT_PAGES {
            let group_repos = match self
                .get_org_repos(org_name, &GITLAB_LIMIT_PER_PAGE, &page_count)
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    log::error!("Can't fetch repos of '{org_name}' (page {page_count}): {err}");
                    break;
                }
            };

            let group_repos_items = match group_repos.as_array() {
                Some(arr) => arr,
                _ => break,
            };

            if group_repos_items.is_empty() {
                break;
            }

            for repo_info in group_repos_items.iter() {
                let repo_name = repo_info["name"].as_str().unwrap_or("");
                let repo_path_with_namespace =
                    repo_info["path_with_namespace"].as_str().unwrap_or("");
                let repo_http_url = repo_info["http_url_to_repo"].as_str().unwrap_or("");

                if repo_name.is_empty() || repo_http_url.is_empty() {
                    continue;
                }

                let repo_owner_login = repo_path_with_namespace
                    .split('/')
                    .next()
                    .unwrap_or(org_name);

                let repo_info_opt = self.clone_or_update_repo(
                    repo_owner_login,
                    repo_name,
                    repo_http_url,
                    branch,
                    dest,
                );

                match repo_info_opt {
                    // `repo_owner_login` is only the first segment of `path_with_namespace`,
                    // so it drops nested subgroups. Restore the real project path here.
                    Some(mut info) => {
                        info.full_path = repo_path_with_namespace.to_string();
                        repos.push(info);
                    }
                    _ => log::info!("'{repo_http_url}' Is not a valid Odoo modules repository!"),
                }
            }

            if group_repos_items.len() < GITLAB_LIMIT_PER_PAGE {
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
        // `with_merge_status_recheck=true` forces GitLab to refresh a stale
        // `detailed_merge_status` synchronously instead of returning a cached
        // "unchecked" - without it, MRs can sit reporting no real status.
        self.request_json(
            format!(
                "projects/{}/merge_requests?state=opened&target_branch={}&per_page={}&page={}&with_merge_status_recheck=true",
                urlencoding::encode(full_path),
                urlencoding::encode(branch),
                per_page,
                page
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
        while page_count < GITLAB_LIMIT_PAGES {
            let merge_requests = match self
                .get_repo_pull_requests(full_path, branch, &GITLAB_LIMIT_PER_PAGE, &page_count)
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    log::error!("Can't fetch merge requests of '{full_path}': {err}");
                    break;
                }
            };
            let mr_items = match merge_requests.as_array() {
                Some(arr) => arr,
                _ => break,
            };
            if mr_items.is_empty() {
                break;
            }
            for mr in mr_items {
                let source_branch = mr["source_branch"].as_str().unwrap_or("");
                if let Some(module_technical_name) = extract_migration_module_name(source_branch) {
                    let created_at = mr["created_at"].as_str().and_then(parse_created_at);
                    let ci_status =
                        detailed_merge_status_to_ci_status(mr["detailed_merge_status"].as_str());
                    prs.push(PullRequestInfo {
                        number: mr["iid"].as_i64().unwrap_or(0),
                        title: mr["title"].as_str().unwrap_or("").to_string(),
                        module_technical_name,
                        created_at,
                        ci_status,
                    });
                }
            }
            if mr_items.len() < GITLAB_LIMIT_PER_PAGE {
                break;
            }
            page_count += 1;
        }
        prs
    }

    async fn is_pull_request_merged(&self, full_path: &str, number: &i64) -> Option<bool> {
        let mr = self
            .request_json(&format!(
                "projects/{}/merge_requests/{number}",
                urlencoding::encode(full_path)
            ))
            .await
            .ok()?;
        Some(mr["state"].as_str() == Some("merged"))
    }
}

/// GitLab's `detailed_merge_status` already folds CI + approvals + conflicts
/// into one value (see https://docs.gitlab.com/ee/api/merge_requests.html),
/// stronger than GitHub's CI-only signal but free (already in the list
/// response) - "mergeable" is the only value meaning fully green.
fn detailed_merge_status_to_ci_status(detailed_status: Option<&str>) -> Option<String> {
    match detailed_status {
        Some("mergeable") => Some("success".to_string()),
        Some("ci_still_running") | Some("checking") | Some("unchecked") => {
            Some("pending".to_string())
        }
        Some(other) if !other.is_empty() => Some("failure".to_string()),
        _ => None,
    }
}
