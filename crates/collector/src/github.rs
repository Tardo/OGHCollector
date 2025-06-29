use std::fs;
use std::path::Path;
use std::process::Command;

const GITHUB_API_VERSION: &'static str = "2022-11-28";
const GITHUB_BASE_URL: &'static str = "https://api.github.com/";
const GITHUB_LIMIT_PER_PAGE: u8 = 50u8;
const GITHUB_LIMIT_PAGES: u8 = 255u8;

pub struct RepoInfo {
    url: String,
    branch: String,
    name: String,
    org: String,
    clone_path: String,
}

impl RepoInfo {
    pub fn get_url(&self) -> &str {
        &self.url
    }
    pub fn get_branch(&self) -> &str {
        &self.branch
    }
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
            Err(e) => panic!("Problem creating the client: {:?}", e)
        };
        Self { token: token.into(), client: client }
    }

    async fn request(&self, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        let full_url = format!("{}{}", GITHUB_BASE_URL, url);
        let res = self.client
            .get(full_url)
            .header(reqwest::header::USER_AGENT, "OGHCollector")
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .send()
            .await?;
        Ok(res)
    }

    async fn request_json(&self, url: &str) -> Result<serde_json::Value, reqwest::Error> {
        let req = self.request(url).await?;
        Ok(req.json().await?)
    }

    pub async fn get_org_repos(&self, org_name: &str, per_page: &u8, page: &u8) -> Result<serde_json::Value, reqwest::Error> {
        let res = self.request_json(format!("orgs/{}/repos?sort=updated&per_page={}&page={}", org_name, per_page, page).as_str()).await?;
        Ok(res)
    }

    pub fn clone_or_update_repo(&self, org_name: &str, repo_name: &str, repo_url: &str, branch: &str, dest: &str) -> Option<RepoInfo> {
        let clone_path = format!("{}/{org_name}/{repo_name}", dest);
        let clone_path_exists = Path::new(&clone_path).exists();
        let git_status: bool;
        if clone_path_exists {
            log::info!("Updating: '{}' in '{}'...", repo_name, clone_path);
            log::info!("Fetch...");
            let output_fetch = Command::new("git")
                        .current_dir(&clone_path)
                        .arg("fetch")
                        .arg("origin")
                        .output()
                        .expect(format!("Can't fetch 'origin'").as_str());
            if !output_fetch.status.success() {
                return None;
            }
            log::info!("Cleaning...");
            let output_clean = Command::new("git")
                        .current_dir(&clone_path)
                        .arg("clean")
                        .arg("-d")
                        .arg("--force")
                        .output()
                        .expect(format!("Can't clean '{}' repository", repo_name).as_str());
            if !output_clean.status.success() {
                return None;
            }
            log::info!("Switch...");
            let output_check = Command::new("git")
                        .current_dir(&clone_path)
                        .arg("switch")
                        .arg("-f")
                        .arg(&branch)
                        .output()
                        .expect(format!("Can't checkout '{}' repository", repo_name).as_str());
            if !output_check.status.success() {
                return None;
            }
            log::info!("Reset...");
            let output_reset = Command::new("git")
                        .current_dir(&clone_path)
                        .arg("reset")
                        .arg("--hard")
                        .arg(format!("origin/{}", &branch).as_str())
                        .output()
                        .expect(format!("Can't checkout '{}' repository", repo_name).as_str());
            if !output_reset.status.success() {
                return None;
            }
            log::info!("Rebase...");
            let output = Command::new("git")
                        .current_dir(&clone_path)
                        .arg("pull")
                        .arg("--rebase")
                        .arg("origin")
                        .arg(&branch)
                        .output()
                        .expect(format!("Can't pull '{}' repository", repo_name).as_str());
            git_status = output.status.success();
            log::info!("Update done!");
        } else {
            log::info!("Cloning: '{}' in '{}'...", repo_name, clone_path);
            match fs::create_dir_all(&clone_path) {
                Ok(res) => res,
                Err(_e) => panic!("Can't create '{}' repository", clone_path) 
            };
            let output = Command::new("git")
                        .current_dir(&clone_path)
                        .arg("clone")
                        .arg(&repo_url)
                        .arg(".")
                        .arg("--depth")
                        .arg("1")
                        .arg("--no-single-branch")
                        .output()
                        .expect(format!("Can't clone '{}' repository", repo_url).as_str());
            if !output.status.success() {
                return None;
            }
            log::info!("Checkout...");
            let output_check = Command::new("git")
                        .current_dir(&clone_path)
                        .arg("checkout")
                        .arg(&branch)
                        .output()
                        .expect(format!("Can't checkout '{}' repository", repo_name).as_str());
            git_status = output_check.status.success();
            log::info!("Clone done!");
        }
        if !git_status {
            return None;
        }
        Some(RepoInfo { url: repo_url.into(), branch: branch.into(), name: repo_name.into(), org: org_name.into(), clone_path })
    }

    pub async fn clone_org_repos(&self, org_name: &str, branch: &str, dest: &str) -> Vec<RepoInfo> {
        let mut page_count: u8 = 1;
        let mut repos: Vec<RepoInfo> = Vec::new();
        while page_count < GITHUB_LIMIT_PAGES {
            let org_repos = self.get_org_repos(&org_name, &GITHUB_LIMIT_PER_PAGE, &page_count).await.unwrap();
            let org_repos_items = org_repos.as_array().unwrap();
            if org_repos_items.len() == 0 {
                break;
            }
            for repo_info in org_repos_items.iter() {
                let repo_owner = repo_info["owner"].as_object().unwrap();
                let repo_owner_login = repo_owner["login"].as_str().unwrap();
                let repo_name = repo_info["name"].as_str().unwrap();
                let repo_url = repo_info["clone_url"].as_str().unwrap();
                let repo_info_opt = self.clone_or_update_repo(&repo_owner_login, &repo_name, &repo_url, &branch, &dest);
                if repo_info_opt.is_none() {
                    log::info!("'{}' Is not a valid Odoo modules repository!", repo_url);
                } else {
                    repos.push(repo_info_opt.unwrap());
                }
            }
            if (org_repos_items.len() as u8) < GITHUB_LIMIT_PER_PAGE {
                break;
            }
            page_count += 1;
        }
        repos
    }
}
