// Copyright 2025 Alexandre D. Díaz
use std::env;
use std::fs;

use oghutils::version::odoo_version_string_to_u8;

#[derive(Debug)]
pub struct OGHCollectorConfig {
    mode: String,
    src: String,
    token: String,
    branch: String,
    repos_path: String,
    version_odoo: u8,
    read_paths: Vec<String>,
}

impl OGHCollectorConfig {
    pub fn new(args: &[String]) -> OGHCollectorConfig {
        let token_file = env::var("OGHCOLLECTOR_TOKEN_FILE").unwrap_or_default();
        let token: String = if token_file.is_empty() {
            env::var("OGHCOLLECTOR_TOKEN").unwrap_or_default()
        } else {
            fs::read_to_string(token_file).unwrap_or_default()
        };
        if token.is_empty() {
            panic!("Need the github api token!")
        }
        let raw_src = args[1].clone();
        let branch = args[2].clone();
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
            version_odoo,
            read_paths,
        }
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

    pub fn get_version_odoo(&self) -> &u8 {
        &self.version_odoo
    }

    pub fn get_read_paths(&self) -> &Vec<String> {
        &self.read_paths
    }
}
