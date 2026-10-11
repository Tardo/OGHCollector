// Copyright Alexandre D. Díaz
use config::Config;
use serde::Deserialize;
use std::collections::HashMap;
use std::env;
use std::fs;

use oghutils::version::{odoo_version_string_to_u8, parse_odoo_version};

// `deny_unknown_fields`: a typo (`path:`, `version:`) in a hand-written or
// generated config must fail, not silently fall back to a default.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectorFile {
    /// Named module-folder lists, so a layout shared by many sources is
    /// written once and referenced by name (`template:`).
    #[serde(default)]
    templates: HashMap<String, Vec<String>>,
    sources: Vec<CollectorSource>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CollectorSource {
    source: String,
    // f64, not String: YAML reads an unquoted `18.0` as a float, which the
    // config crate would stringify as "18" (not a branch).
    versions: Vec<f64>,
    template: Option<String>,
    #[serde(default)]
    paths: Vec<String>,
    #[serde(default = "default_git")]
    git: String,
    #[serde(default)]
    base_url: String,
}

fn default_git() -> String {
    "GH".to_string()
}

/// Expands a collector config (see the shipped `collector.yaml`) into the
/// `<source>[:<paths>] <version> <git>` argument lists of the single-run CLI,
/// in file order. Everything is validated here, before any job runs.
pub fn load_jobs(path: &str) -> Result<Vec<Vec<String>>, String> {
    let CollectorFile { templates, sources } = Config::builder()
        .add_source(config::File::with_name(path))
        .build()
        .and_then(|c| c.try_deserialize())
        .map_err(|err| format!("Can't load the collector config '{path}': {err}"))?;
    let mut jobs = Vec::new();
    for (idx, src) in sources.into_iter().enumerate() {
        let name = format!("sources[{idx}] ({})", src.source);
        // ':' and ',' separate the source and its paths in the CLI argument.
        if src.source.is_empty() || src.source.contains([':', ',']) {
            return Err(format!("{name}: invalid source"));
        }
        let git = src.git.to_uppercase();
        if git != "GH" && git != "GL" {
            return Err(format!("{name}: unknown git '{}' (GH or GL)", src.git));
        }
        // The GitHub client always talks to github.com.
        if git == "GH" && !src.base_url.is_empty() {
            return Err(format!("{name}: base_url is only for GitLab (git: GL)"));
        }
        if src.versions.is_empty() {
            return Err(format!("{name}: no versions"));
        }
        let mut paths = match &src.template {
            Some(tmpl) => templates
                .get(tmpl)
                .cloned()
                .ok_or_else(|| format!("{name}: unknown template '{tmpl}'"))?,
            None => Vec::new(),
        };
        paths.extend(src.paths);
        let mut origin = src.source.clone();
        if !paths.is_empty() {
            let mut normalized = Vec::with_capacity(paths.len());
            for p in &paths {
                if p.contains([':', ',']) {
                    return Err(format!("{name}: invalid path '{p}'"));
                }
                normalized.push(match p.trim_matches('/') {
                    "" | "." => String::new(),
                    p => format!("/{p}"),
                });
            }
            origin = format!("{origin}:{}", normalized.join(","));
        }
        let git = if src.base_url.is_empty() {
            git
        } else {
            format!("{git}:{}", src.base_url)
        };
        for ver in &src.versions {
            let ver = format!("{ver:.1}");
            if parse_odoo_version(&ver).is_none() {
                return Err(format!("{name}: invalid Odoo version '{ver}'"));
            }
            jobs.push(vec![origin.clone(), ver, git.clone()]);
        }
    }
    Ok(jobs)
}

#[derive(Debug)]
pub enum GitType {
    Github,
    Gitlab,
}

#[derive(Debug)]
pub struct OGHCollectorConfig {
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
        let raw_src_parts = raw_src.split(":").collect::<Vec<&str>>();
        let src = raw_src_parts[0];

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

#[cfg(test)]
mod tests {
    #[test]
    fn shipped_collector_yaml_matches_the_old_ofelia_loop() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../collector.yaml");
        let job = |src: &str, ver: &str| vec![src.to_string(), ver.to_string(), "GH".to_string()];
        let new_versions = [
            "10.0", "11.0", "12.0", "13.0", "14.0", "15.0", "16.0", "17.0", "18.0", "19.0", "20.0",
        ];
        let old_versions = ["6.1", "7.0", "8.0", "9.0"];
        let mut expected: Vec<Vec<String>> = Vec::new();
        expected.extend(old_versions.map(|v| job("odoo/odoo:/addons,/openerp/addons", v)));
        expected.extend(new_versions.map(|v| job("odoo/odoo:/addons,/odoo/addons", v)));
        expected.extend(
            old_versions
                .iter()
                .chain(&new_versions)
                .map(|v| job("OCA", v)),
        );
        assert_eq!(super::load_jobs(path).unwrap(), expected);
    }
}
