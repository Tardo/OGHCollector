use crate::clients::github::GithubClient;
use crate::clients::gitlab::GitlabClient;
use crate::gitclient::{GitClient, RepoInfo};

pub enum AnyGitClient {
    Github(GithubClient),
    Gitlab(GitlabClient),
}

impl GitClient for AnyGitClient {
    fn new(_token: &str) -> Self {
        unreachable!("Use AnyGitClient::Github(...) or AnyGitClient::Gitlab(...) directly")
    }

    async fn request(&self, url: &str) -> Result<reqwest::Response, reqwest::Error> {
        match self {
            AnyGitClient::Github(c) => c.request(url).await,
            AnyGitClient::Gitlab(c) => c.request(url).await,
        }
    }

    async fn request_json(&self, url: &str) -> Result<serde_json::Value, reqwest::Error> {
        match self {
            AnyGitClient::Github(c) => c.request_json(url).await,
            AnyGitClient::Gitlab(c) => c.request_json(url).await,
        }
    }

    async fn get_org_repos(
        &self,
        org_name: &str,
        per_page: &usize,
        page: &usize,
    ) -> Result<serde_json::Value, reqwest::Error> {
        match self {
            AnyGitClient::Github(c) => c.get_org_repos(org_name, per_page, page).await,
            AnyGitClient::Gitlab(c) => c.get_org_repos(org_name, per_page, page).await,
        }
    }

    async fn clone_org_repos(&self, org_name: &str, branch: &str, dest: &str) -> Vec<RepoInfo> {
        match self {
            AnyGitClient::Github(c) => c.clone_org_repos(org_name, branch, dest).await,
            AnyGitClient::Gitlab(c) => c.clone_org_repos(org_name, branch, dest).await,
        }
    }
}
