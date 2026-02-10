// Copyright 2025 Alexandre D. Díaz
use duct::cmd;
use std::fs;
use std::path::Path;

const GITHUB_API_VERSION: &str = "2022-11-28";
const GITHUB_BASE_URL: &str = "https://api.github.com/";
const GITHUB_LIMIT_PER_PAGE: usize = 50;
const GITHUB_LIMIT_PAGES: usize = 255;

pub struct RepoInfo {
    name: String,
    org: String,
    clone_path: String,
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
}

#[derive(Debug)]
pub struct GithubClient {
    token: String,
    client: reqwest::Client,
}

impl GithubClient {
    pub fn new(token: &str) -> Self {
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

    pub async fn get_org_repos(
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

    pub fn clone_or_update_repo(
        &self,
        org_name: &str,
        repo_name: &str,
        repo_url: &str,
        branch: &str,
        dest: &str,
    ) -> Option<RepoInfo> {
        let clone_path = format!("{dest}/{org_name}/{repo_name}");
        let clone_path_exists = Path::new(&clone_path).exists();
        if clone_path_exists {
            log::info!("Updating repo: {repo_name} @ {branch}");
            cmd!("git", "fetch", "origin", "--prune")
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            cmd!("git", "reset", "--hard", &format!("origin/{branch}"))
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            cmd!("git", "clean", "-fdx")
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            cmd!("git", "switch", "-C", branch, &format!("origin/{branch}"))
                .dir(&clone_path)
                .stdin_null()
                .run()
                .ok()?;
            log::info!("Repo updated & cleaned: {repo_name} @ {branch}");
        } else {
            log::info!("Cloning repo: {repo_name} @ {branch}");
            if fs::create_dir_all(&clone_path).is_err() {
                log::error!("Cannot create directory: {clone_path}");
                return None;
            }

            cmd!(
                "git",
                "clone",
                "--no-single-branch",
                "--branch",
                branch,
                repo_url,
                ".",
            )
            .dir(&clone_path)
            .stdin_null()
            .run()
            .ok()?;
        }
        Some(RepoInfo {
            name: repo_name.into(),
            org: org_name.into(),
            clone_path,
        })
    }

    pub async fn clone_org_repos(&self, org_name: &str, branch: &str, dest: &str) -> Vec<RepoInfo> {
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
                    None => log::info!("'{repo_url}' Is not a valid Odoo modules repository!"),
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
