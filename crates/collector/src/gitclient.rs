// Copyright Alexandre D. Díaz
use duct::cmd;
use std::fs;
use std::path::Path;

pub struct RepoInfo {
    pub name: String,
    pub org: String,
    pub clone_path: String,
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

pub trait GitClient {
    fn new(token: &str) -> Self;

    fn clone_or_update_repo(
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
            let base_dir = format!("{dest}/{org_name}");
            if fs::create_dir_all(&base_dir).is_err() {
                log::error!("Cannot create directory: {base_dir}");
                return None;
            }

            cmd!(
                "git",
                "clone",
                "--no-single-branch",
                "--branch",
                branch,
                repo_url,
            )
            .dir(&base_dir)
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
}
