// Copyright Alexandre D. Díaz
// Adapted for GitLab
use crate::gitclient::{GitClient, RepoInfo};

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
            let group_repos = self
                .get_org_repos(org_name, &GITLAB_LIMIT_PER_PAGE, &page_count)
                .await
                .unwrap();

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
                    Some(info) => repos.push(info),
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
}
