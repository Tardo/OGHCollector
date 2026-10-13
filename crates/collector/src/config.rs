// Copyright Alexandre D. Díaz
use config::Config;
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
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
    /// Portal workspaces its repos are shown in (default: all of them).
    #[serde(default)]
    workspaces: Vec<String>,
    /// `migration` (default): only OCA-named port branches, no analysis.
    /// `full`: every PR/MR, mapped to its modules and analyzed.
    #[serde(default = "default_pull_requests")]
    pull_requests: String,
}

/// Values of a source's `pull_requests` (the 5th job argument).
pub const PULL_REQUEST_MODES: [&str; 2] = ["migration", "full"];

fn default_pull_requests() -> String {
    PULL_REQUEST_MODES[0].to_string()
}

/// The 4th job argument meaning "shown in every workspace" (it can't be a
/// workspace name). Without a 4th argument a job leaves workspaces untouched.
pub const ALL_WORKSPACES: &str = "all";

fn is_valid_workspace(ws: &str) -> bool {
    // Goes into a cookie and a SQL literal on the portal side.
    ws != ALL_WORKSPACES
        && !ws.is_empty()
        && ws
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn default_git() -> String {
    "GH".to_string()
}

/// Expands a collector config (see the shipped `collector.yaml`) into the
/// `<source>[:<paths>] <version> <git> <workspaces> <pull_requests>` argument lists of the
/// single-run CLI, in file order. Everything is validated here, before any
/// job runs.
pub fn load_jobs(path: &str) -> Result<Vec<Vec<String>>, String> {
    let CollectorFile { templates, sources } = Config::builder()
        .add_source(config::File::with_name(path))
        .build()
        .and_then(|c| c.try_deserialize())
        .map_err(|err| format!("Can't load the collector config '{path}': {err}"))?;
    // Each job rewrites the workspaces of the repos it collects, so entries
    // with the same `source` (e.g. odoo/odoo per template) get the union, or
    // they'd overwrite each other; any entry without `workspaces` = all.
    // ponytail: an org and one of its repos (`OCA` + `OCA/web`) are different
    // sources, the last job to collect the repo wins; merge by repo if needed.
    let mut source_workspaces: HashMap<&str, Option<BTreeSet<&str>>> = HashMap::new();
    for (idx, src) in sources.iter().enumerate() {
        if let Some(ws) = src.workspaces.iter().find(|ws| !is_valid_workspace(ws)) {
            return Err(format!(
                "sources[{idx}] ({}): invalid workspace '{ws}' (letters, digits, '-' or '_'; \
                 not '{ALL_WORKSPACES}')",
                src.source
            ));
        }
        let entry = source_workspaces
            .entry(&src.source)
            .or_insert_with(|| Some(BTreeSet::new()));
        match (entry.as_mut(), src.workspaces.is_empty()) {
            (Some(_), true) => *entry = None,
            (Some(set), false) => set.extend(src.workspaces.iter().map(String::as_str)),
            (None, _) => {}
        }
    }
    let source_workspaces: HashMap<String, String> = source_workspaces
        .into_iter()
        .map(|(src, ws)| {
            let ws = ws.map_or(ALL_WORKSPACES.to_string(), |ws| {
                ws.into_iter().collect::<Vec<_>>().join(",")
            });
            (src.to_string(), ws)
        })
        .collect();
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
        if !PULL_REQUEST_MODES.contains(&src.pull_requests.as_str()) {
            return Err(format!(
                "{name}: unknown pull_requests '{}' ({})",
                src.pull_requests,
                PULL_REQUEST_MODES.join(" or ")
            ));
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
            jobs.push(vec![
                origin.clone(),
                ver,
                git.clone(),
                source_workspaces[&src.source].clone(),
                src.pull_requests.clone(),
            ]);
        }
    }
    Ok(jobs)
}

/// A nonempty Docker secret `/run/secrets/<file>`, else the env var, trimmed.
pub fn read_secret(file: &str, env_var: &str) -> Option<String> {
    let secret = fs::read_to_string(format!("/run/secrets/{file}")).unwrap_or_default();
    let secret = match secret.trim() {
        "" => env::var(env_var).unwrap_or_default().trim().to_string(),
        secret => secret.to_string(),
    };
    (!secret.is_empty()).then_some(secret)
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
    workspaces: Option<Vec<String>>,
    full_pull_requests: bool,
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

        // `None`: no 4th argument, leave the repos' workspaces as they are.
        let workspaces = args.get(4).map(|arg| match arg.as_str() {
            ALL_WORKSPACES => Vec::new(),
            arg => {
                let ws: Vec<String> = arg.split(',').map(str::to_string).collect();
                if let Some(bad) = ws.iter().find(|ws| !is_valid_workspace(ws)) {
                    panic!("Invalid workspace '{bad}'");
                }
                ws
            }
        });

        // No 5th argument: `migration`, the cheap one.
        let full_pull_requests = match args.get(5).map(String::as_str) {
            None | Some("migration") => false,
            Some("full") => true,
            Some(bad) => panic!("Invalid pull_requests '{bad}' (migration or full)"),
        };

        OGHCollectorConfig {
            src: src.to_string(),
            token,
            branch,
            repos_path,
            base_url,
            git_type,
            version_odoo,
            read_paths,
            workspaces,
            full_pull_requests,
        }
    }

    fn read_token(git_orig: &str) -> String {
        let file = format!("{}_token", git_orig.to_lowercase());
        let env_var = format!("OGHCOLLECTOR_TOKEN_{git_orig}");
        read_secret(&file, &env_var)
            .unwrap_or_else(|| panic!("No API token: set /run/secrets/{file} or {env_var}"))
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

    pub fn get_workspaces(&self) -> Option<&Vec<String>> {
        self.workspaces.as_ref()
    }

    /// Every PR/MR analyzed (`full`), not only migration branches tracked.
    pub fn get_full_pull_requests(&self) -> bool {
        self.full_pull_requests
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn shipped_collector_yaml_matches_the_old_ofelia_loop() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../collector.yaml");
        let job = |src: &str, ver: &str| {
            vec![
                src.to_string(),
                ver.to_string(),
                "GH".to_string(),
                "all".to_string(),
                "migration".to_string(),
            ]
        };
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

    #[test]
    fn workspaces_are_merged_per_source() {
        let dir = std::env::temp_dir().join(format!("ogh-ws-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("collector.yaml");
        let load = |yaml: &str| {
            std::fs::write(&path, yaml).unwrap();
            super::load_jobs(path.to_str().unwrap())
        };
        let ws_of =
            |jobs: Vec<Vec<String>>| jobs.into_iter().map(|j| j[3].clone()).collect::<Vec<_>>();
        let jobs = load(
            "sources:\n\
             - {source: A, versions: [18.0], workspaces: [y, x]}\n\
             - {source: A, versions: [17.0], workspaces: [z]}\n\
             - {source: B, versions: [18.0], workspaces: [x]}\n\
             - {source: B, versions: [17.0]}\n",
        );
        assert_eq!(ws_of(jobs.unwrap()), ["x,y,z", "x,y,z", "all", "all"]);
        let jobs = load("sources:\n- {source: A, versions: [18.0], pull_requests: full}\n");
        assert_eq!(jobs.unwrap()[0][4], "full");
        assert!(load("sources:\n- {source: A, versions: [18.0], pull_requests: deep}\n").is_err());
        for bad in ["all", "a b", "a'b", "''"] {
            let yaml =
                format!("sources:\n- {{source: A, versions: [18.0], workspaces: [{bad}]}}\n");
            assert!(load(&yaml).is_err(), "{bad}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
