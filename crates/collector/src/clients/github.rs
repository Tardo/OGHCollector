// Copyright Alexandre D. Díaz
use crate::gitclient::{GitClient, RepoInfo};

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
            let org_repos = self
                .get_org_repos(org_name, &GITHUB_LIMIT_PER_PAGE, &page_count)
                .await
                .unwrap();
            let org_repos_items = org_repos.as_array().unwrap();
            if org_repos_items.is_empty() {
                break;
            }
            for repo_info in org_repos_items.iter() {
                let repo_owner = repo_info["owner"].as_object().unwrap();
                let repo_owner_login = repo_owner["login"].as_str().unwrap();
                let repo_name = repo_info["name"].as_str().unwrap();
                let repo_url = repo_info["clone_url"].as_str().unwrap();
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
}
