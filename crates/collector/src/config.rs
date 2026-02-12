// Copyright Alexandre D. Díaz
use std::env;
use std::fs;

use oghutils::version::odoo_version_string_to_u8;

#[derive(Debug)]
pub enum GitType {
    Github,
    Gitlab,
}

#[derive(Debug)]
pub struct OGHCollectorConfig {
    mode: String,
    src: String,
    token: String,
    branch: String,
    repos_path: String,
    base_url: String,
    git_type: GitType,
    version_odoo: u8,
    read_paths: Vec<String>,
}

impl OGHCollectorConfig {
    pub fn new(args: &[String]) -> OGHCollectorConfig {
        let raw_src = args[1].clone();
        let branch = args[2].clone();
        let raw_git_type = args.get(3).map_or("GH", |s| s.as_str());
        let git_type_str: String;
        let base_url: String;
        if let Some((gtyp, burl)) = raw_git_type.split_once(':') {
            git_type_str = gtyp.to_uppercase();
            base_url = burl.to_string();
        } else {
            git_type_str = raw_git_type.to_uppercase();
            base_url = String::new();
        }
        let git_type = match git_type_str.as_str() {
            "GL" => GitType::Gitlab,
            _ => GitType::Github,
        };
        let token = OGHCollectorConfig::read_token(git_type_str.as_str());
        let current_path = env::current_dir().unwrap();
        let repos_path = format!("{}/data/repos", current_path.display());
        let branch_parts = branch.split(".").collect::<Vec<&str>>();
        let version_odoo: u8 = if branch_parts.len() == 1 {
            odoo_version_string_to_u8(branch_parts[0])
        } else {
            odoo_version_string_to_u8(&branch_parts[..2].join("."))
        };
        let mut mode: String = "org".to_string();

        let raw_src_parts = raw_src.split(":").collect::<Vec<&str>>();
        let src = raw_src_parts[0];
        if src.contains('/') {
            mode = "repo".to_string();
        }

        let mut read_paths: Vec<String>;
        if raw_src_parts.len() == 2 {
            read_paths = Vec::new();
            let read_paths_str = raw_src_parts[1].split(",").collect::<Vec<&str>>();
            for path_str in read_paths_str {
                read_paths.push(path_str.to_string());
            }
        } else {
            read_paths = vec!["".to_string()];
        }

        OGHCollectorConfig {
            mode,
            src: src.to_string(),
            token,
            branch,
            repos_path,
            base_url,
            git_type,
            version_odoo,
            read_paths,
        }
    }

    fn read_token(git_orig: &str) -> String {
        let git_orig_lower = git_orig.to_lowercase();
        let secret_path = format!("/run/secrets/{git_orig_lower}_token");

        if let Ok(content) = fs::read_to_string(&secret_path) {
            let token = content.trim().to_string();
            if !token.is_empty() {
                return token;
            }
        }

        let env_var = format!("OGHCOLLECTOR_TOKEN_{git_orig}");
        let token = env::var(env_var).unwrap_or_default().trim().to_string();

        if token.is_empty() {
            panic!("Need the github api token!");
        }
        token
    }

    pub fn get_mode(&self) -> &String {
        &self.mode
    }

    pub fn get_source(&self) -> &String {
        &self.src
    }

    pub fn get_token(&self) -> &String {
        &self.token
    }

    pub fn get_branch(&self) -> &String {
        &self.branch
    }

    pub fn get_repos_path(&self) -> &String {
        &self.repos_path
    }

    pub fn get_base_url(&self) -> &String {
        &self.base_url
    }

    pub fn get_version_odoo(&self) -> &u8 {
        &self.version_odoo
    }

    pub fn get_read_paths(&self) -> &Vec<String> {
        &self.read_paths
    }

    pub fn get_git_type(&self) -> &GitType {
        &self.git_type
    }
}
