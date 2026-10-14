// Copyright Alexandre D. Díaz
// Adapted for GitLab
use crate::gitclient::{parse_created_at, GitClient, PullRequestInfo, RepoInfo};

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
        let client = crate::gitclient::http_client();
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

    // GitLab takes any non-blank user name with a personal/group/project
    // access token, and `oauth2` with an OAuth one.
    fn git_credentials(&self) -> (&str, &str) {
        ("oauth2", &self.token)
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

            // API errors are `Err` (see `GitClient::request_json`); this is a
            // 2xx that isn't the project array.
            let group_repos_items = match group_repos.as_array() {
                Some(arr) => arr,
                _ => {
                    log::error!(
                        "Unexpected GitLab response for '{org_name}': {}",
                        group_repos["message"]
                    );
                    break;
                }
            };

            if group_repos_items.is_empty() {
                break;
            }

            for project in group_repos_items.iter() {
                repos.extend(self.clone_project(project, org_name, branch, dest));
            }

            if group_repos_items.len() < GITLAB_LIMIT_PER_PAGE {
                break;
            }

            page_count += 1;
        }

        repos
    }

    /// GitLab nests groups, so `a/b` is either a project or a subgroup: the
    /// project lookup decides, anything that isn't one is listed as a group.
    async fn clone_source(&self, source: &str, branch: &str, dest: &str) -> Vec<RepoInfo> {
        if source.contains('/') {
            let url = format!("projects/{}", urlencoding::encode(source));
            match self.request_json(&url).await {
                Ok(project) if project["http_url_to_repo"].is_string() => {
                    return self
                        .clone_project(&project, source, branch, dest)
                        .into_iter()
                        .collect();
                }
                // Not a project: listed as a (sub)group below.
                Ok(_) => {}
                Err(err) if err.status() == Some(reqwest::StatusCode::NOT_FOUND) => {}
                Err(err) => {
                    log::error!("Can't look up '{source}': {err}");
                    return Vec::new();
                }
            }
        }
        self.clone_org_repos(source, branch, dest).await
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

    async fn get_open_pull_requests(
        &self,
        full_path: &str,
        branch: &str,
    ) -> Option<Vec<PullRequestInfo>> {
        let mut prs: Vec<PullRequestInfo> = Vec::new();
        for page in 1..=GITLAB_LIMIT_PAGES {
            let merge_requests = match self
                .get_repo_pull_requests(full_path, branch, &GITLAB_LIMIT_PER_PAGE, &page)
                .await
            {
                Ok(res) => res,
                Err(err) => {
                    log::error!("Can't fetch merge requests of '{full_path}': {err}");
                    return None;
                }
            };
            let Some(mr_items) = merge_requests.as_array() else {
                log::error!("Unexpected GitLab response for the merge requests of '{full_path}'");
                return None;
            };
            for mr in mr_items {
                prs.push(PullRequestInfo {
                    number: mr["iid"].as_i64().unwrap_or(0),
                    title: mr["title"].as_str().unwrap_or("").to_string(),
                    head_ref: mr["source_branch"].as_str().unwrap_or("").to_string(),
                    description: mr["description"].as_str().unwrap_or("").to_string(),
                    head_sha: mr["sha"].as_str().unwrap_or("").to_string(),
                    author: mr["author"]["username"].as_str().map(str::to_string),
                    is_draft: mr["draft"].as_bool().unwrap_or(false),
                    labels: mr["labels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|l| l.as_str().map(str::to_string))
                        .collect(),
                    created_at: mr["created_at"].as_str().and_then(parse_created_at),
                    last_message_at: mr["updated_at"].as_str().and_then(parse_created_at),
                    ci_status: detailed_merge_status_to_ci_status(
                        mr["detailed_merge_status"].as_str(),
                    ),
                });
            }
            if mr_items.len() < GITLAB_LIMIT_PER_PAGE {
                return Some(prs);
            }
        }
        // A partial list would close every MR past the cap.
        log::error!("'{full_path}' has more open merge requests than the page cap");
        None
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

impl GitlabClient {
    /// One project of a `projects`/`groups/:id/projects` response. The
    /// organization is the first segment of `path_with_namespace` (the whole
    /// path is kept as `full_path`, which the merge request API needs).
    /// ponytail: same-named projects in different subgroups of one group share
    /// `<group>/<name>` (clone dir and DB repo); collect at most one of them
    /// until org/repo can hold a nested namespace.
    /// The repo name is the project `path` (URL slug, like a GitHub repo name),
    /// not its display `name` ("Sale Addons"): it keys the clone dir and
    /// doodba's addons.yaml/repos.yaml.
    fn clone_project(
        &self,
        project: &serde_json::Value,
        org_name: &str,
        branch: &str,
        dest: &str,
    ) -> Option<RepoInfo> {
        let name = project["path"].as_str().unwrap_or("");
        let path_with_namespace = project["path_with_namespace"].as_str().unwrap_or("");
        let http_url = project["http_url_to_repo"].as_str().unwrap_or("");
        if name.is_empty() || http_url.is_empty() {
            return None;
        }
        let owner = path_with_namespace
            .split_once('/')
            .map_or(org_name, |(owner, _)| owner);
        let Some(mut info) = self.clone_or_update_repo(owner, name, http_url, branch, dest) else {
            log::info!("'{http_url}' Is not a valid Odoo modules repository!");
            return None;
        };
        info.full_path = path_with_namespace.to_string();
        Some(info)
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
