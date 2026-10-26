// Copyright Alexandre D. Díaz
//! Doodba projects: a collected branch with `odoo/custom/src/repos.yaml` and `addons.yaml`
//! (or a copier `_src_path` naming a doodba template) is one. Per job, after the modules,
//! `collect` reads each repo's project files and stores them in `doodba_project` (+ `_addon`),
//! or deletes what was stored when the branch isn't one (anymore):
//! - `.copier-answers.yml`: `_src_path`/`_commit` and the `ANSWERS` allowlist only (the rest
//!   carries client domains, backup destinations, SMTP settings).
//! - `repos.yaml`: remotes, target and the merges after the first one (`patches`: PRs/MRs and
//!   branches carried on top of the base branch; git-aggregator merges onto `merges[0]`).
//! - `addons.yaml`: repo → addons (every document; `ENV`/`ONLY` aren't repos).
//! - `.pre-commit-config.yaml` hook revs, `dependencies/*.txt`, the Dockerfile `FROM`.
//!
//! Template drift: each distinct template is cloned once per job under
//! `<repos_path>/.templates/<host>/<path>` and `_commit` (copier writes `git describe --tags
//! --always`: `v1.2-3-gabc1234` → `abc1234`) is compared with its default branch. Credentials go
//! only to the job's own forge host (`on_forge`); a commit the template doesn't have is
//! "unknown", not an error. These files hold secrets (repos.yaml remotes with `user:token@`):
//! `strip_credentials` runs on every string stored, and server/mcp are unauthenticated. Where
//! they hold them is stored (`exposed`: file and host, never the credential; of the answers only
//! `_src_path`) so it can be fixed.
//!
//! Patches that are PRs/MRs get their state (`PatchStates`): GitHub ones through the job's
//! client or, in a GitLab job, one built from the GitHub token when there is one; MRs only on
//! the job's own forge. Asked once per job each, at most `PATCH_LOOKUPS_PER_JOB`; `merged` is
//! final. Exact pip pins (`pkg==1.2`) get PyPI's advisories, once per job each. A lookup that
//! can't be made (no token, out of lookups, a failure) keeps the stored state/advisories, so an
//! outage never reads as "nothing". Per-repo failures `warn` and keep what was stored; only a DB
//! write failure is an error.
use regex::Regex;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use yaml_rust2::{Yaml, YamlLoader};

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models::dependency_osv;
use sqlitedb::models::doodba_project::{self, Advisory, Linter, Patch, Repo};
use sqlitedb::DbSqliteConnection as SqliteConnection;

use crate::anygitclient::AnyGitClient;
use crate::clients::github::GithubClient;
use crate::gitclient::{git_auth_env, git_run, git_run_env, GitClient, RepoInfo};
use crate::pypi::PypiClient;

/// Copier answers worth showing; anything else stays out of the DB.
const ANSWERS: &[&str] = &[
    "odoo_version",
    "postgres_version",
    "odoo_proxy",
    "traefik_version",
    "compose_version",
    "project_license",
];
const DEPENDENCY_KINDS: &[&str] = &["apt_build", "apt", "gem", "npm", "pip"];
/// Template commits listed per project (the count has them all).
const TEMPLATE_LOG_CAP: usize = 50;
/// PR/MR state lookups per job (each distinct PR once).
const PATCH_LOOKUPS_PER_JOB: usize = 300;

#[derive(Debug, Default)]
pub(crate) struct Project {
    pub template_src: Option<String>,
    pub template_commit: Option<String>,
    pub answers: Map<String, Value>,
    pub base_image: Option<String>,
    pub repos: Vec<Repo>,
    pub addons: Vec<(String, String)>,
    pub linters: Vec<Linter>,
    pub dependencies: BTreeMap<String, Vec<String>>,
    pub exposed: Vec<String>,
}

static CREDENTIALS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([A-Za-z][A-Za-z0-9+.-]*://)[^/@\s]+@").unwrap());
static SCP_URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\w.-]+@([\w.-]+):/?(.+)$").unwrap());
/// An http(s) URL with userinfo; `$VAR` placeholders (git-aggregator
/// `--expand-env`) aren't credentials, nor is the `git@` of ssh/scp URLs.
static HTTP_CREDENTIALS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bhttps?://[^/@\s$]+@([^/:\s]+)").unwrap());
/// `name[extras]==version`, up to a marker or a comment.
static PIP_PIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([A-Za-z0-9][A-Za-z0-9._-]*)\s*(?:\[[^\]]*\])?\s*==\s*([A-Za-z0-9][A-Za-z0-9.+!_-]*)\s*(?:[;#].*)?$",
    )
    .unwrap()
});

/// `https://user:token@host/x` → `https://host/x`, anywhere in `text`.
pub(crate) fn strip_credentials(text: &str) -> String {
    CREDENTIALS.replace_all(text, "$1").into_owned()
}

/// "<file> <host>" of each http(s) URL with credentials in the project files
/// read here: where they are, never what. Of the copier answers only
/// `src_path` (`_src_path`): the rest names hosts the allowlist keeps out.
fn exposed(root: &Path, src_path: Option<&str>) -> Vec<String> {
    let files = ["odoo/custom/src/repos.yaml".to_string()]
        .into_iter()
        .chain(
            DEPENDENCY_KINDS
                .iter()
                .map(|kind| format!("odoo/custom/dependencies/{kind}.txt")),
        );
    let texts = src_path
        .map(|src| (".copier-answers.yml".to_string(), src.to_string()))
        .into_iter()
        .chain(files.map(|file| {
            let text = fs::read_to_string(root.join(&file)).unwrap_or_default();
            (file, text)
        }));
    let mut out: Vec<String> = Vec::new();
    for (file, text) in texts {
        for caps in HTTP_CREDENTIALS.captures_iter(&text) {
            let found = format!("{file} {}", &caps[1]);
            if !out.contains(&found) {
                out.push(found);
            }
        }
    }
    out
}

/// (package, version) of an exact pip pin.
fn pip_pin(line: &str) -> Option<(String, String)> {
    let caps = PIP_PIN.captures(line)?;
    Some((caps[1].to_string(), caps[2].to_string()))
}

/// A remote as an https URL without credentials (`git@host:a/b.git` →
/// `https://host/a/b.git`), so links and PR URLs can be built from it.
fn clean_url(url: &str) -> String {
    let url = strip_credentials(url.trim());
    SCP_URL.replace(&url, "https://$1/$2").into_owned()
}

fn scalar(yaml: &Yaml) -> Option<String> {
    match yaml {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

/// The documents of a YAML file, `None` when it doesn't exist.
fn read_yaml(path: &Path) -> Result<Option<Vec<Yaml>>, String> {
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    match fs::read_to_string(path) {
        Ok(src) => YamlLoader::load_from_str(&src)
            .map(Some)
            .map_err(|err| format!("{name}: {err}")),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("{name}: {err}")),
    }
}

fn hash_entries(yaml: &Yaml) -> impl Iterator<Item = (String, &Yaml)> {
    yaml.as_hash()
        .into_iter()
        .flatten()
        .filter_map(|(k, v)| Some((scalar(k)?, v)))
}

/// `remote ref` (a string or `{remote, ref}`), `$ODOO_VERSION` substituted.
fn merge(yaml: &Yaml, version: &str) -> Option<Patch> {
    let text = match yaml {
        Yaml::Hash(_) => format!("{} {}", scalar(&yaml["remote"])?, scalar(&yaml["ref"])?),
        other => scalar(other)?,
    };
    let text = text
        .replace("${ODOO_VERSION}", version)
        .replace("$ODOO_VERSION", version);
    let mut parts = text.split_whitespace();
    Some(Patch {
        remote: parts.next()?.to_string(),
        git_ref: parts.next()?.to_string(),
        state: None,
    })
}

fn parse_repos(doc: &Yaml, version: &str) -> Vec<Repo> {
    hash_entries(doc)
        .map(|(path, entry)| Repo {
            path: path.trim_start_matches("./").to_string(),
            target: merge(&entry["target"], version).map(|t| format!("{} {}", t.remote, t.git_ref)),
            remotes: hash_entries(&entry["remotes"])
                .filter_map(|(name, url)| Some((name, clean_url(&scalar(url)?))))
                .collect(),
            patches: entry["merges"]
                .as_vec()
                .into_iter()
                .flatten()
                .skip(1)
                .filter_map(|m| merge(m, version))
                .map(|p| Patch {
                    git_ref: strip_credentials(&p.git_ref),
                    ..p
                })
                .collect(),
        })
        .collect()
}

fn parse_addons(docs: &[Yaml]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (repo, addons) in docs.iter().flat_map(hash_entries) {
        if repo == "ENV" || repo == "ONLY" {
            continue;
        }
        let names = match addons {
            Yaml::Array(list) => list.iter().filter_map(scalar).collect(),
            other => scalar(other).into_iter().collect::<Vec<_>>(),
        };
        out.extend(names.into_iter().map(|name| (repo.clone(), name)));
    }
    out
}

/// Reads the project files of a checkout; `Ok(None)` = not a doodba project.
pub(crate) fn detect(root: &Path, version: &str) -> Result<Option<Project>, String> {
    let src = root.join("odoo/custom/src");
    let answers = read_yaml(&root.join(".copier-answers.yml"))?;
    let answers = answers.as_ref().and_then(|docs| docs.first());
    let src_path = answers.and_then(|a| scalar(&a["_src_path"]));
    let template_src = src_path.as_deref().map(strip_credentials);
    let repos = read_yaml(&src.join("repos.yaml"))?;
    let addons = read_yaml(&src.join("addons.yaml"))?;
    let doodba_template = template_src
        .as_deref()
        .is_some_and(|s| s.contains("doodba"));
    if !(repos.is_some() && addons.is_some() || doodba_template) {
        return Ok(None);
    }
    let mut project = Project {
        template_commit: answers.and_then(|a| scalar(&a["_commit"])),
        template_src,
        answers: ANSWERS
            .iter()
            .filter_map(|key| {
                let value = scalar(&answers?[*key])?;
                Some((key.to_string(), Value::String(strip_credentials(&value))))
            })
            .collect(),
        repos: repos
            .iter()
            .flatten()
            .flat_map(|doc| parse_repos(doc, version))
            .collect(),
        addons: parse_addons(addons.as_deref().unwrap_or_default()),
        exposed: exposed(root, src_path.as_deref()),
        ..Default::default()
    };
    if let Some(docs) = read_yaml(&root.join(".pre-commit-config.yaml"))? {
        project.linters = docs
            .iter()
            .flat_map(|doc| doc["repos"].as_vec().into_iter().flatten())
            .filter_map(|hook| {
                Some(Linter {
                    repo: clean_url(&scalar(&hook["repo"])?),
                    rev: scalar(&hook["rev"])?,
                })
            })
            .collect();
    }
    for kind in DEPENDENCY_KINDS {
        let path = root.join(format!("odoo/custom/dependencies/{kind}.txt"));
        let lines: Vec<String> = fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(strip_credentials)
            .collect();
        if !lines.is_empty() {
            project.dependencies.insert(kind.to_string(), lines);
        }
    }
    project.base_image = fs::read_to_string(root.join("odoo/Dockerfile"))
        .unwrap_or_default()
        .lines()
        .find_map(|l| {
            let (from, image) = l.trim().split_once(char::is_whitespace)?;
            from.eq_ignore_ascii_case("FROM").then(|| {
                image
                    .trim()
                    .replace("${ODOO_VERSION}", version)
                    .replace("$ODOO_VERSION", version)
            })
        });
    Ok(Some(project))
}

/// A copier `_src_path` as a clonable https URL (`gh:`/`gl:` shorthands, scp
/// form); `None` for a local path.
fn template_url(src: &str) -> Option<String> {
    let url = if let Some(path) = src.strip_prefix("gh:") {
        format!("https://github.com/{path}")
    } else if let Some(path) = src.strip_prefix("gl:") {
        format!("https://gitlab.com/{path}")
    } else {
        clean_url(src)
    };
    reqwest::Url::parse(&url)
        .ok()
        .filter(|u| matches!(u.scheme(), "https" | "http"))
        .map(|_| url)
}

/// Template drift of `commit` in a template checkout: commits on the default
/// branch since it (`None` = the template doesn't have it), the newest
/// `TEMPLATE_LOG_CAP` of them and the commit's date.
pub(crate) fn drift(dir: &str, commit: &str) -> (Option<i32>, Vec<String>, Option<String>) {
    // `git describe` output: the commit is after `-g`.
    let hash = commit
        .rsplit_once("-g")
        .filter(|(_, h)| h.chars().all(|c| c.is_ascii_hexdigit()))
        .map_or(commit, |(_, h)| h);
    let range = format!("{hash}..origin/HEAD");
    let known = git_run(
        dir,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{hash}^{{commit}}"),
        ],
    );
    let behind: Option<i32> = known
        .is_ok()
        .then(|| {
            git_run(dir, &["rev-list", "--count", &range])
                .ok()?
                .parse()
                .ok()
        })
        .flatten();
    if behind.is_none() {
        return (None, Vec::new(), None);
    }
    let cap = format!("-{TEMPLATE_LOG_CAP}");
    let log = git_run(dir, &["log", &cap, "--format=%h %s", &range]).unwrap_or_default();
    let date = git_run(dir, &["show", "-s", "--format=%cI", hash]).ok();
    (behind, log.lines().map(str::to_string).collect(), date)
}

/// `url` is on the job's forge (`forge_url`'s origin), over https: only
/// then does our token go with it.
fn on_forge(url: &reqwest::Url, forge_url: &str) -> bool {
    url.scheme() == "https"
        && reqwest::Url::parse(forge_url).is_ok_and(|f| f.origin() == url.origin())
}

/// The client to ask about a PR/MR of the remote `url`, and its path there:
/// github.com through a GitHub client (`job` or `github`), anything else only
/// through `job` on our own forge.
fn pr_api<'c>(
    job: &'c AnyGitClient,
    github: Option<&'c AnyGitClient>,
    url: &str,
    forge_url: &str,
) -> Option<(&'c AnyGitClient, String)> {
    static SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\w[\w.-]*$").unwrap());
    let parsed = reqwest::Url::parse(url).ok()?;
    let path = parsed.path().trim_matches('/').trim_end_matches(".git");
    let segments: Vec<&str> = path.split('/').collect();
    // It goes into an authenticated API URL.
    if !segments.iter().all(|s| SEGMENT.is_match(s)) || parsed.scheme() != "https" {
        return None;
    }
    let client = match (parsed.host_str() == Some("github.com"), job) {
        (true, _) if segments.len() != 2 => return None,
        (true, AnyGitClient::Github(_)) => job,
        (true, AnyGitClient::Gitlab(_)) => github?,
        (false, AnyGitClient::Gitlab(_)) if on_forge(&parsed, forge_url) => job,
        _ => return None,
    };
    Some((client, path.to_string()))
}

/// The job's PR/MR state lookups, by `<remote url>#<number>`.
struct PatchStates<'a> {
    job: &'a AnyGitClient,
    /// GitHub's client in a GitLab job, when there's a GitHub token.
    github: Option<AnyGitClient>,
    seen: HashMap<String, Option<String>>,
    lookups_left: usize,
}

impl<'a> PatchStates<'a> {
    fn new(job: &'a AnyGitClient) -> Self {
        let github = match job {
            AnyGitClient::Github(_) => None,
            AnyGitClient::Gitlab(_) => {
                crate::config::read_secret("gh_token", "OGHCOLLECTOR_TOKEN_GH")
                    .filter(|token| !token.is_empty())
                    .map(|token| AnyGitClient::Github(GithubClient::new(&token, "")))
            }
        };
        Self {
            job,
            github,
            seen: HashMap::new(),
            lookups_left: PATCH_LOOKUPS_PER_JOB,
        }
    }

    async fn state(&mut self, url: &str, number: i64, forge_url: &str) -> Option<String> {
        let key = format!("{}#{number}", url.trim_end_matches(".git"));
        if let Some(state) = self.seen.get(&key) {
            return state.clone();
        }
        let (client, path) = pr_api(self.job, self.github.as_ref(), url, forge_url)?;
        if self.lookups_left == 0 {
            return None;
        }
        self.lookups_left -= 1;
        if self.lookups_left == 0 {
            log::warn!("{PATCH_LOOKUPS_PER_JOB} patch PR/MR lookups done, the rest stay unknown");
        }
        let state = client.pull_request_state(&path, &number).await;
        self.seen.insert(key, state.clone());
        state
    }

    /// Sets the state of each PR/MR patch of `repos`. What `stored` (the
    /// previous row's repos) says stands when it can't be asked (no token,
    /// out of lookups, a failure), and `merged` is final.
    async fn fill(&mut self, repos: &mut [Repo], stored: &[Repo], forge_url: &str) {
        let known: HashMap<(&str, &str, &str), &str> = stored
            .iter()
            .flat_map(|r| r.patches.iter().map(move |p| (r, p)))
            .filter_map(|(r, p)| {
                let key = (r.path.as_str(), p.remote.as_str(), p.git_ref.as_str());
                Some((key, p.state.as_deref()?))
            })
            .collect();
        for repo in repos {
            for patch in &mut repo.patches {
                let (Some(number), Some(url)) = (patch.number(), repo.remotes.get(&patch.remote))
                else {
                    continue;
                };
                let key = (
                    repo.path.as_str(),
                    patch.remote.as_str(),
                    patch.git_ref.as_str(),
                );
                let before = known.get(&key).map(|s| s.to_string());
                patch.state = match before.as_deref() {
                    Some("merged") => before,
                    _ => self.state(url, number, forge_url).await.or(before),
                };
            }
        }
    }
}

/// The advisories of a pin (`package` lowercased, version): PyPI's answer,
/// or what `stored` had for it when PyPI couldn't be asked (`None`), so an
/// outage doesn't read as "no advisories".
fn pin_advisories(
    found: Option<&[Advisory]>,
    stored: &[Advisory],
    (package, version): &(String, String),
) -> Vec<Advisory> {
    match found {
        Some(found) => found.to_vec(),
        None => stored
            .iter()
            .filter(|a| a.package.to_lowercase() == *package && a.version == *version)
            .cloned()
            .collect(),
    }
}

/// PyPI's advisories of the exact pins in `pip`, each pin asked once per job
/// (`seen`, `None` = the lookup failed); `stored` = the previous row's.
async fn advisories(
    pypi: &PypiClient,
    pip: &[String],
    stored: &[Advisory],
    seen: &mut HashMap<(String, String), Option<Vec<Advisory>>>,
) -> Vec<Advisory> {
    let mut out = Vec::new();
    for (package, version) in pip.iter().filter_map(|line| pip_pin(line)) {
        let key = (package.to_lowercase(), version.clone());
        if !seen.contains_key(&key) {
            let found = match pypi.vulnerabilities(&package, &version).await {
                Ok(vulns) => Some(
                    vulns
                        .into_iter()
                        .map(|v| Advisory {
                            summary: dependency_osv::summary(&v.details, &package),
                            package: package.clone(),
                            version: version.clone(),
                            id: v.id,
                            fixed_in: v.fixed_in,
                        })
                        .collect(),
                ),
                Err(err) => {
                    log::warn!("Can't query PyPI for '{package}=={version}', keeping what was stored: {err}");
                    None
                }
            };
            seen.insert(key.clone(), found);
        }
        out.extend(pin_advisories(seen[&key].as_deref(), stored, &key));
    }
    out
}

/// The job's template checkouts, by URL (`None` = couldn't get it).
struct Templates<'a> {
    dir: PathBuf,
    git: &'a AnyGitClient,
    checkouts: HashMap<String, Option<String>>,
}

impl Templates<'_> {
    fn checkout(&mut self, url: &str, forge_url: &str) -> Option<String> {
        if let Some(dir) = self.checkouts.get(url) {
            return dir.clone();
        }
        let parsed = reqwest::Url::parse(url).ok()?;
        let path = parsed.path().trim_matches('/').trim_end_matches(".git");
        let dir = self
            .dir
            .join(parsed.host_str().unwrap_or_default())
            .join(path);
        let dir_str = dir.to_string_lossy().to_string();
        // A template elsewhere is fetched anonymously.
        let auth = match on_forge(&parsed, forge_url) {
            true => {
                let (user, token) = self.git.git_credentials();
                git_auth_env(url, user, token)
            }
            false => Vec::new(),
        };
        // ponytail: two jobs of different orgs fetching one template at once
        // collide on git's lock and warn; they never run at once in the daemon.
        let got = if dir.join(".git").exists() {
            git_run_env(&dir_str, &["fetch", "--prune", "origin"], &auth)
        } else {
            // Cloned by name from its parent, so a relative `repos_path` works.
            let parent = dir
                .parent()
                .unwrap_or(&self.dir)
                .to_string_lossy()
                .to_string();
            let name = dir
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            fs::create_dir_all(&parent)
                .map_err(|err| err.to_string())
                .and_then(|_| git_run_env(&parent, &["clone", "--quiet", url, &name], &auth))
                .inspect_err(|_| {
                    let _ = fs::remove_dir_all(&dir);
                })
        };
        let got = match got {
            Ok(_) => Some(dir_str),
            // A stale checkout still tells how far behind a project was.
            Err(err) if dir.join(".git").exists() => {
                log::warn!("Can't update the template '{url}', using what was fetched: {err}");
                Some(dir_str)
            }
            Err(err) => {
                log::warn!("Can't get the template '{url}': {err}");
                None
            }
        };
        self.checkouts.insert(url.to_string(), got.clone());
        got
    }
}

pub(crate) async fn collect(
    conn: &mut SqliteConnection,
    git: &AnyGitClient,
    pypi: &PypiClient,
    repos: &[RepoInfo],
    gh_repo_ids: &[i64],
    version_odoo: u8,
    repos_path: &str,
) {
    let version = odoo_version_u8_to_string(&version_odoo);
    let mut templates = Templates {
        dir: Path::new(repos_path).join(".templates"),
        git,
        checkouts: HashMap::new(),
    };
    let mut patch_states = PatchStates::new(git);
    let mut pins = HashMap::new();
    for (repo, gh_repo_id) in repos.iter().zip(gh_repo_ids) {
        let mut project = match detect(Path::new(repo.get_clone_path()), &version) {
            Ok(project) => project,
            Err(err) => {
                log::warn!(
                    "'{}': can't read its doodba files, keeping what was stored: {err}",
                    repo.get_full_path()
                );
                continue;
            }
        };
        let mut model = None;
        if let Some(p) = project.as_mut() {
            // What was stored stands where it can't be asked again.
            let stored = doodba_project::find(conn, *gh_repo_id, version_odoo as i32);
            let stored_repos: Vec<Repo> = stored
                .as_ref()
                .and_then(|m| serde_json::from_str(&m.repos).ok())
                .unwrap_or_default();
            let stored_advisories: Vec<Advisory> = stored
                .as_ref()
                .and_then(|m| serde_json::from_str(&m.advisories).ok())
                .unwrap_or_default();
            patch_states
                .fill(&mut p.repos, &stored_repos, repo.get_url())
                .await;
            let pip = p.dependencies.get("pip").map_or(&[][..], Vec::as_slice);
            let advisories = advisories(pypi, pip, &stored_advisories, &mut pins).await;
            let (behind, log, date) = p
                .template_src
                .as_deref()
                .and_then(template_url)
                .zip(p.template_commit.as_deref())
                .and_then(|(url, commit)| {
                    let dir = templates.checkout(&url, repo.get_url())?;
                    Some(drift(&dir, commit))
                })
                .unwrap_or_default();
            if !p.exposed.is_empty() {
                log::warn!(
                    "'{}': credentials in its project files: {}",
                    repo.get_full_path(),
                    p.exposed.join(", ")
                );
            }
            let patches: Vec<&Patch> = p.repos.iter().flat_map(|r| &r.patches).collect();
            log::info!(
                "'{}': doodba project, {} addons, {} patches ({} merged or closed), {} advisories, template {}",
                repo.get_full_path(),
                p.addons.len(),
                patches.len(),
                patches.iter().filter(|p| p.is_done()).count(),
                advisories.len(),
                behind.map_or("drift unknown".to_string(), |n| format!(
                    "{n} commits behind"
                ))
            );
            model = Some(doodba_project::Model {
                gh_repository_id: *gh_repo_id,
                version_odoo: version_odoo as i32,
                last_commit_date: git_run(repo.get_clone_path(), &["log", "-1", "--format=%cI"])
                    .unwrap_or_default(),
                template_src: p.template_src.clone(),
                template_commit: p.template_commit.clone(),
                template_commit_date: date,
                template_behind: behind,
                template_log: Value::from(log).to_string(),
                answers: Value::Object(p.answers.clone()).to_string(),
                base_image: p.base_image.clone(),
                repos: serde_json::to_string(&p.repos).unwrap_or_default(),
                linters: serde_json::to_string(&p.linters).unwrap_or_default(),
                dependencies: serde_json::to_string(&p.dependencies).unwrap_or_default(),
                exposed: serde_json::to_string(&p.exposed).unwrap_or_default(),
                advisories: serde_json::to_string(&advisories).unwrap_or_default(),
            });
        }
        let stored = doodba_project::replace(
            conn,
            *gh_repo_id,
            version_odoo as i32,
            model.as_ref().zip(project.as_ref().map(|p| &p.addons[..])),
        );
        if let Err(err) = stored {
            log::error!(
                "'{}': can't store the doodba project: {err}",
                repo.get_full_path()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::gitlab::GitlabClient;
    use duct::cmd;

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn patch(remote: &str, git_ref: &str) -> Patch {
        Patch {
            remote: remote.into(),
            git_ref: git_ref.into(),
            state: None,
        }
    }

    #[test]
    fn detects_a_project_and_never_keeps_credentials() {
        let root = std::env::temp_dir().join(format!("ogh-doodba-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        assert!(detect(&root, "16.0").unwrap().is_none());
        write(
            &root,
            ".copier-answers.yml",
            "_commit: v9.6.1-3-gfd15475\n_src_path: https://bot:s3cret@forge.example/tpl/doodba-copier-template.git\n\
             odoo_version: 16.0\npostgres_version: \"15\"\nbackup_dst: rsync://x@backup.example//p\n\
             smtp_relay: https://mailer:pw@mail.example/\n",
        );
        write(
            &root,
            "odoo/custom/src/repos.yaml",
            "./odoo:\n  remotes:\n    ocb: https://github.com/OCA/OCB.git\n  target: ocb $ODOO_VERSION\n  merges:\n    - ocb $ODOO_VERSION\n\
             web:\n  remotes:\n    origin: git@github.com:OCA/web.git\n    ssh: ssh://git@github.com/OCA/web.git\n  target: origin $ODOO_VERSION\n  merges:\n    - origin $ODOO_VERSION\n    - origin refs/pull/2779/head # fix\n    - {remote: origin, ref: \"${ODOO_VERSION}-mig\"}\n\
             private_external:\n  remotes:\n    origin: https://infra:-Zeys7Ax@forge.example/p/x.git\n    env: https://oauth2:$GL_TOKEN@forge.example/p/y.git\n  merges:\n    - origin $ODOO_VERSION\n",
        );
        write(
            &root,
            "odoo/custom/src/addons.yaml",
            "web:\n  - web_responsive\n  - web_refresher\nprivate: \"*\"\n---\nENV:\n  PGDATABASE: prod\nONLY:\n  PGDATABASE: [prod]\nserver-tools: [base_technical_user]\n",
        );
        write(
            &root,
            ".pre-commit-config.yaml",
            "repos:\n  - repo: local\n    hooks: []\n  - repo: https://github.com/OCA/pylint-odoo\n    rev: v8.0.20\n",
        );
        write(
            &root,
            "odoo/custom/dependencies/pip.txt",
            "# comment\nxlrd\ngit+https://u:tok@forge.example/x.git@16.0\n\n",
        );
        write(
            &root,
            "odoo/Dockerfile",
            "ARG ODOO_VERSION\nFROM ghcr.io/tecnativa/doodba:${ODOO_VERSION}-onbuild\n",
        );

        let p = detect(&root, "16.0").unwrap().unwrap();
        assert_eq!(p.template_commit.as_deref(), Some("v9.6.1-3-gfd15475"));
        assert_eq!(
            p.template_src.as_deref(),
            Some("https://forge.example/tpl/doodba-copier-template.git")
        );
        assert_eq!(p.answers.len(), 2, "allowlist only: {:?}", p.answers);
        assert_eq!(p.answers["odoo_version"], "16.0");
        assert_eq!(p.repos.len(), 3);
        assert_eq!(p.repos[0].path, "odoo");
        assert!(p.repos[0].patches.is_empty());
        assert_eq!(
            p.repos[1].remotes["origin"],
            "https://github.com/OCA/web.git"
        );
        assert_eq!(p.repos[1].target.as_deref(), Some("origin 16.0"));
        assert_eq!(
            p.repos[1].patches,
            [
                patch("origin", "refs/pull/2779/head"),
                patch("origin", "16.0-mig")
            ]
        );
        assert_eq!(p.repos[1].patches[0].number(), Some(2779));
        assert_eq!(p.repos[1].patches[1].number(), None);
        assert_eq!(
            p.repos[2].remotes["origin"],
            "https://forge.example/p/x.git"
        );
        assert_eq!(
            p.addons,
            [
                ("web".into(), "web_responsive".into()),
                ("web".into(), "web_refresher".into()),
                ("private".into(), "*".into()),
                ("server-tools".into(), "base_technical_user".into()),
            ]
        );
        assert_eq!(
            p.linters,
            [Linter {
                repo: "https://github.com/OCA/pylint-odoo".into(),
                rev: "v8.0.20".into()
            }]
        );
        assert_eq!(
            p.dependencies["pip"],
            ["xlrd", "git+https://forge.example/x.git@16.0"]
        );
        assert_eq!(
            p.base_image.as_deref(),
            Some("ghcr.io/tecnativa/doodba:16.0-onbuild")
        );
        // Where, not what: no ssh `git@`, no `$GL_TOKEN` placeholder.
        assert_eq!(
            p.exposed,
            [
                ".copier-answers.yml forge.example",
                "odoo/custom/src/repos.yaml forge.example",
                "odoo/custom/dependencies/pip.txt forge.example",
            ]
        );
        let stored = format!("{p:?}");
        for secret in [
            "s3cret",
            "Zeys7Ax",
            "tok@",
            "backup.example",
            "mail.example",
        ] {
            assert!(!stored.contains(secret), "{secret} leaked: {stored}");
        }
        assert_eq!(
            template_url("gh:Tecnativa/doodba-copier-template").as_deref(),
            Some("https://github.com/Tecnativa/doodba-copier-template")
        );
        assert_eq!(template_url("/home/me/template"), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn only_exact_pip_pins_are_looked_up() {
        let pin = |line: &str| pip_pin(line).map(|(p, v)| format!("{p} {v}"));
        assert_eq!(pin("xlrd==1.2.0").as_deref(), Some("xlrd 1.2.0"));
        assert_eq!(
            pin("Pillow[webp] == 9.0.1 ; python_version >= '3.8'").as_deref(),
            Some("Pillow 9.0.1")
        );
        assert_eq!(
            pin("requests==2.31.0 # pinned").as_deref(),
            Some("requests 2.31.0")
        );
        for line in [
            "xlrd",
            "xlrd>=1.2",
            "xlrd==1.*",
            "xlrd===1.2",
            "-r other.txt",
            "git+https://github.com/x/y.git@16.0#egg=y",
            "y @ https://example.com/y-1.0.tar.gz",
        ] {
            assert_eq!(pin(line), None, "{line}");
        }
    }

    #[tokio::test]
    async fn patch_states_only_ask_the_forge_they_belong_to() {
        let gitlab =
            AnyGitClient::Gitlab(GitlabClient::new("tok", "https://forge.example/api/v4/"));
        let github = AnyGitClient::Github(GithubClient::new("tok", ""));
        let forge = "https://forge.example/g/project.git";
        let api = |job, github, url| {
            pr_api(job, github, url, forge).map(|(c, path)| {
                let gh = matches!(c, AnyGitClient::Github(_));
                (if gh { "gh" } else { "gl" }, path)
            })
        };
        // A GitLab job: its MRs, GitHub PRs only with a GitHub token.
        assert_eq!(
            api(&gitlab, None, "https://forge.example/g/sub/p.git"),
            Some(("gl", "g/sub/p".into()))
        );
        assert_eq!(api(&gitlab, None, "https://github.com/OCA/web.git"), None);
        assert_eq!(
            api(&gitlab, Some(&github), "https://github.com/OCA/web.git"),
            Some(("gh", "OCA/web".into()))
        );
        for url in [
            "http://forge.example/g/p.git",
            "https://other.example/g/p.git",
            "https://github.com/OCA/web/extra.git",
            "https://forge.example/g/p%2F..%2Fx.git",
        ] {
            assert_eq!(api(&gitlab, Some(&github), url), None, "{url}");
        }
        // A GitHub job: only GitHub.
        assert_eq!(
            api(&github, None, "https://github.com/OCA/web"),
            Some(("gh", "OCA/web".into()))
        );
        assert_eq!(api(&github, None, "https://forge.example/g/p.git"), None);

        // A merged patch stays merged without asking; out of lookups, the
        // rest keep what was stored, or stay unknown (no network here).
        let mut states = PatchStates {
            job: &gitlab,
            github: None,
            seen: HashMap::new(),
            lookups_left: 0,
        };
        let repo = |states: [Option<&str>; 4]| Repo {
            path: "web".into(),
            target: None,
            remotes: BTreeMap::from([("origin".into(), "https://forge.example/g/web.git".into())]),
            patches: [
                "refs/merge-requests/7/head",
                "refs/merge-requests/8/head",
                "refs/merge-requests/9/head",
                "16.0-fix",
            ]
            .iter()
            .zip(states)
            .map(|(git_ref, state)| Patch {
                state: state.map(String::from),
                ..patch("origin", git_ref)
            })
            .collect(),
        };
        let mut repos = [repo([None; 4])];
        let stored = [repo([Some("merged"), Some("closed"), None, None])];
        states.fill(&mut repos, &stored, forge).await;
        let got: Vec<_> = repos[0]
            .patches
            .iter()
            .map(|p| p.state.as_deref())
            .collect();
        assert_eq!(got, [Some("merged"), Some("closed"), None, None]);
    }

    #[test]
    fn a_failed_pypi_lookup_keeps_the_stored_advisories() {
        let advisory = |package: &str, version: &str| Advisory {
            package: package.into(),
            version: version.into(),
            id: format!("GHSA-{package}"),
            fixed_in: "9.0".into(),
            summary: String::new(),
        };
        let stored = [advisory("PyOpenSSL", "24.1.0"), advisory("lxml", "4.0")];
        let pin = ("pyopenssl".to_string(), "24.1.0".to_string());
        assert_eq!(pin_advisories(None, &stored, &pin), [stored[0].clone()]);
        // PyPI answered: its answer, even when it's none.
        assert!(pin_advisories(Some(&[]), &stored, &pin).is_empty());
        let bumped = ("pyopenssl".to_string(), "26.0.0".to_string());
        assert!(pin_advisories(None, &stored, &bumped).is_empty());
    }

    #[test]
    fn drift_counts_template_commits_since_the_project_one() {
        let pid = std::process::id();
        let root = std::env::temp_dir().join(format!("ogh-doodba-tpl-{pid}"));
        // Relative, as a `repos_path` could be: tests run in the crate dir.
        let templates_dir = PathBuf::from(format!("../../target/ogh-doodba-templates-{pid}"));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&templates_dir);
        let upstream = root.join("upstream");
        fs::create_dir_all(&upstream).unwrap();
        let git = |dir: &Path, args: &[&str]| {
            cmd("git", args)
                .dir(dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "Ana")
                .env("GIT_AUTHOR_EMAIL", "ana@example.com")
                .env("GIT_COMMITTER_NAME", "Ana")
                .env("GIT_COMMITTER_EMAIL", "ana@example.com")
                .stderr_null()
                .read()
                .unwrap()
        };
        git(&upstream, &["init", "-q", "-b", "main"]);
        for subject in ["first", "second", "third"] {
            git(&upstream, &["commit", "-q", "--allow-empty", "-m", subject]);
        }
        let first = git(&upstream, &["rev-list", "--max-parents=0", "HEAD"]);
        let forge = AnyGitClient::Gitlab(GitlabClient::new("tok", "https://forge.example/api/v4/"));
        let mut templates = Templates {
            dir: templates_dir.join(".templates"),
            git: &forge,
            checkouts: HashMap::new(),
        };
        let url = format!("file://{}", upstream.display());
        let dir = templates
            .checkout(&url, "https://forge.example/g/p.git")
            .expect("fresh clone");
        assert!(Path::new(&dir).join(".git").exists(), "{dir}");
        // Fetched once per job: the second project gets the same checkout.
        assert_eq!(
            templates
                .checkout(&url, "https://forge.example/g/p.git")
                .as_deref(),
            Some(dir.as_str())
        );
        let dir = dir.as_str();

        let (behind, log, date) = drift(dir, &format!("v1.0-0-g{}", &first[..7]));
        assert_eq!(behind, Some(2));
        assert_eq!(log.len(), 2);
        assert!(log[0].ends_with(" third"), "{log:?}");
        assert!(date.is_some());
        assert_eq!(drift(dir, "deadbee"), (None, Vec::new(), None));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&templates_dir);
    }
}
