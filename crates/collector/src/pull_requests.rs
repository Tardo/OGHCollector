// Copyright Alexandre D. Díaz
//! Open PRs/MRs of a collected repository and version. Per source
//! (`collector.yaml` `pull_requests`):
//! - `migration` (default; OCA/Odoo-style public data): only PRs on an OCA
//!   port branch (`{version}-mig-{module}`), stored with their CI, nothing
//!   fetched nor analyzed.
//! - `full` (your own repos): every PR, stored with CI, labels and head,
//!   mapped to the modules it changes (a local `git diff`, no API calls), and
//!   each changed module analyzed at the PR head the same way a merged one is
//!   (manifest, security findings, migration notes).
//!
//! PR code is untrusted (anyone's fork): it's never checked out in the shared
//! clone, only its changed modules are extracted (`git archive`, symlinks
//! dropped) into a work directory of their own, and manifests are parsed with
//! `ast.literal_eval`.
//!
//! Change events (`system_event`) are only recorded against a known previous
//! state, or for PRs opened in the last day, so a first run over thousands of
//! open PRs stays quiet. This is where future workflows hook in.
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use duct::cmd;
use regex::Regex;
use serde_json::{json, Value};
use sqlitedb::models::pull_request::{Model as PullRequest, NewPullRequest};
use sqlitedb::models::pull_request_module::NewPullRequestModule;
use sqlitedb::models::{self, system_event};
use sqlitedb::DbSqliteConnection as SqliteConnection;

use crate::analyzer::{self, OGHCollectorAnalyzer};
use crate::anygitclient::AnyGitClient;
use crate::gitclient::{git_run, GitClient, RepoInfo};
use crate::workflow::{Deps, Engine, FoundPr};
use crate::{migration, security};

/// Modules analyzed per PR at most; the rest still get their row (analysis
/// `None`).
// ponytail: a mass refactor PR shows only its first modules' findings.
const MAX_ANALYZED_MODULES: usize = 30;

const MANIFESTS: [&str; 2] = ["__manifest__.py", "__openerp__.py"];

/// GitHub CI lookups (2 requests each) per job at most: the first run sees
/// every open PR's head as new, and one job must not burn the hourly rate
/// limit the daemon's webhook jobs share. The rest are looked up later.
pub const CI_LOOKUPS_PER_JOB: usize = 500;

/// What one job may still spend on PRs.
pub struct Budget {
    pub analyses: usize,
    pub ci_lookups: usize,
}

/// One repository/version of the running job.
pub struct RepoPrs<'a> {
    pub git: &'a AnyGitClient,
    pub repo: &'a RepoInfo,
    pub gh_repo_id: i64,
    pub branch: &'a str,
    pub version_odoo: u8,
    pub read_paths: &'a [String],
    pub analyzer: &'a OGHCollectorAnalyzer,
    /// This job's own work directory (see `main`).
    pub workdir: &'a Path,
    /// The source's `pull_requests: full`.
    pub full: bool,
    /// Its `on: pull_request` workflows, run on every tracked PR at the end.
    pub engine: &'a Engine,
    pub deps: &'a Deps<'a>,
}

/// OCA migration convention: head branch `{version}-mig-{module_technical_name}`,
/// optionally followed by a `-{discard}` suffix (e.g. `16.0-mig-sale_commission-wip`).
/// Returns the module technical name if it matches.
///
/// Odoo technical names never contain `-`, only `_`. So if the captured tail
/// already has an `_` in it, it's a real name with one discard segment tacked
/// on the end - drop everything after the last `-`. If it has no `_` at all,
/// the whole thing was typed with `-` instead of `_` and there's no way to
/// tell a discard suffix apart from the name - just normalize every `-` to `_`.
/// ponytail: ambiguous when both a hyphenated name AND a discard suffix are
/// present (e.g. `sale-order-line-wip`) - the discard ends up folded into the
/// name. Add a real module-name lookup if this proves common in practice.
pub fn extract_migration_module_name(head_ref: &str) -> Option<String> {
    static RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[0-9]+\.[0-9]+-mig-(.+)$").unwrap());
    RE.captures(head_ref).and_then(|caps| caps.get(1)).map(|m| {
        let raw = m.as_str();
        if raw.contains('_') {
            raw.rsplit_once('-')
                .map_or(raw, |(name, _discard)| name)
                .to_string()
        } else {
            raw.replace('-', "_")
        }
    })
}

fn is_final(ci_status: Option<&str>) -> bool {
    matches!(ci_status, Some("success" | "failure"))
}

/// Stores the open PRs, closes the gone ones and analyzes the PRs whose head
/// (or the analysis rules) changed, most recently active first, while
/// `budget` lasts: the rest keep their old analysis until a later run.
// ponytail: an org job spends it repo by repo in collection order, so late
// repos wait for later runs; share it by recency across repos if that matters.
pub async fn collect(c: &RepoPrs<'_>, conn: &mut SqliteConnection, budget: &mut Budget) {
    let full_path = c.repo.get_full_path();
    let repo_name = c.repo.get_name();
    let Some(listed) = c.git.get_open_pull_requests(full_path, c.branch).await else {
        // Unknown (already logged as an error), not "all closed": keep the
        // stored PRs instead of closing every one of them.
        return;
    };
    let mut open: Vec<PullRequest> = Vec::with_capacity(listed.len());
    // (listing, row before this run, row now) of every tracked PR.
    let mut tracked = Vec::with_capacity(listed.len());
    for info in &listed {
        let migration_module = extract_migration_module_name(&info.head_ref);
        if !c.full && migration_module.is_none() {
            continue;
        }
        let prev = models::pull_request::get_by_repository_prid(conn, &c.gh_repo_id, &info.number)
            .unwrap();
        let prev_ci = prev.as_ref().and_then(|p| p.ci_status.clone());
        let head_changed =
            prev.as_ref().and_then(|p| p.head_sha.as_deref()) != Some(&info.head_sha);
        // GitHub's listing has no CI: two requests per PR, so only for a new
        // head or a state that can still change.
        let needs_ci = head_changed || !is_final(prev_ci.as_deref());
        let ci_status = match &info.ci_status {
            Some(ci) => Some(ci.clone()),
            None if needs_ci && budget.ci_lookups > 0 => {
                budget.ci_lookups -= 1;
                c.git.get_ci_status(full_path, &info.head_sha).await
            }
            // Not looked up yet: unknown, not the previous head's.
            None if head_changed => None,
            None => prev_ci,
        };
        let labels = serde_json::to_string(&info.labels).ok();
        let (before, pr) = models::pull_request::add(
            conn,
            &NewPullRequest {
                name: &info.title,
                version_odoo: c.version_odoo as i32,
                prid: info.number,
                gh_repository_id: c.gh_repo_id,
                created_at: info.created_at.as_deref(),
                ci_status: ci_status.as_deref(),
                last_message_at: info.last_message_at.as_deref(),
                head_sha: Some(info.head_sha.as_str()).filter(|sha| !sha.is_empty()),
                author: info.author.as_deref(),
                is_draft: info.is_draft,
                labels: labels.as_deref(),
            },
        )
        .unwrap();
        // `migration` sources: the module comes from the branch name, and only
        // the (old) migration events are logged.
        if let (false, Some(module), None) = (c.full, &migration_module, &before) {
            models::pull_request_module::replace_for_pull_request(
                conn,
                pr.id,
                &[NewPullRequestModule {
                    pull_request_id: pr.id,
                    module_technical_name: module,
                    is_new: true,
                    analysis: None,
                }],
            )
            .unwrap();
            let _ = system_event::register_new_migration_pr(
                conn,
                module,
                &pr.name,
                &pr.prid,
                repo_name,
                &c.version_odoo,
            );
        }
        if let (true, Some(before), Some(ci)) = (c.full, &before, pr.ci_status.as_deref()) {
            if is_final(Some(ci)) && before.ci_status.as_deref() != Some(ci) {
                let _ = system_event::register_pull_request_ci(
                    conn,
                    &pr.name,
                    &pr.prid,
                    repo_name,
                    &c.version_odoo,
                    ci,
                );
            }
        }
        tracked.push((info, before, pr.clone()));
        open.push(pr);
    }

    let prids: Vec<i64> = open.iter().map(|pr| pr.prid).collect();
    let listed_prids: HashSet<i64> = listed.iter().map(|info| info.number).collect();
    // Still open, only no longer tracked (the source went `full` →
    // `migration`): dropped quietly, they weren't closed.
    let (untracked, closed): (Vec<PullRequest>, Vec<PullRequest>) =
        models::pull_request::find_outdated(conn, &c.gh_repo_id, &c.version_odoo, &prids)
            .unwrap_or_default()
            .into_iter()
            .partition(|pr| listed_prids.contains(&pr.prid));
    if !untracked.is_empty() {
        let ids: Vec<i64> = untracked.iter().map(|pr| pr.id).collect();
        let _ = models::pull_request::delete_by_ids(conn, &ids);
    }
    let mut merged_prids: Vec<i64> = Vec::with_capacity(closed.len());
    for pr in &closed {
        if let Some(true) = c.git.is_pull_request_merged(full_path, &pr.prid).await {
            merged_prids.push(pr.prid);
        }
    }
    let _ = models::pull_request::delete_outdated(
        conn,
        &c.gh_repo_id,
        &c.version_odoo,
        &prids,
        &merged_prids,
    );

    if c.full {
        analyze_pending(c, conn, budget, open);
    }
    if !c.engine.runs("pull_request") {
        return;
    }
    for (info, before, pr) in tracked {
        // `diff()` needs the head: `migration` sources (or PRs the analysis
        // budget skipped) never fetched it. Once fetched, it stays.
        let clone = c.repo.get_clone_path();
        if git_run(clone, &["cat-file", "-e", &info.head_sha]).is_err() {
            if let Err(err) = c.git.fetch_pull_request(clone, c.repo.get_url(), pr.prid) {
                log::warn!(
                    "PR #{} of '{}': fetch failed: {err}",
                    pr.prid,
                    c.repo.get_name()
                );
            }
        }
        let modules = models::pull_request_module::get_by_pull_request_ids(conn, &[pr.id])
            .unwrap_or_default();
        let found = FoundPr {
            repo: c.repo,
            version_odoo: c.version_odoo,
            branch: c.branch,
            info,
            pr,
            before,
            modules,
        };
        c.engine.run_pull_request(&found, conn, c.deps).await;
    }
}

/// Analyzes the PRs whose head (or the analysis rules) changed, most
/// recently active first, while `budget` lasts.
fn analyze_pending(
    c: &RepoPrs,
    conn: &mut SqliteConnection,
    budget: &mut Budget,
    mut open: Vec<PullRequest>,
) {
    let repo_name = c.repo.get_name();
    open.retain(|pr| {
        pr.head_sha.is_some()
            && (pr.analyzed_sha != pr.head_sha || pr.analysis_version != analyzer::ANALYSIS_VERSION)
    });
    open.sort_by(|a, b| b.last_message_at.cmp(&a.last_message_at));
    let pending = open.len();
    for (done, pr) in open.iter().enumerate() {
        if budget.analyses == 0 {
            log::info!(
                "PR analysis limit reached: {} PR(s) of '{repo_name}' left for a later run",
                pending - done
            );
            break;
        }
        budget.analyses -= 1;
        analyze(c, conn, pr);
    }
}

/// Changed module folders of a PR (repo-relative path → whether any change
/// can alter the analysis: `.py`, `.xml`, `.csv`) from its changed files: a
/// file counts when it's `<read path>/<folder>/...`. Whether the folder is a
/// module is checked against the PR head afterwards.
fn touched_modules(files: &[&str], read_paths: &[String]) -> BTreeMap<String, bool> {
    let mut out: BTreeMap<String, bool> = BTreeMap::new();
    for file in files {
        for read_path in read_paths {
            let prefix = read_path.trim_matches('/');
            let rest = match prefix {
                "" => Some(*file),
                _ => file.strip_prefix(prefix).and_then(|r| r.strip_prefix('/')),
            };
            let Some((folder, inner)) = rest.and_then(|r| r.split_once('/')) else {
                continue;
            };
            let dir = match prefix {
                "" => folder.to_string(),
                _ => format!("{prefix}/{folder}"),
            };
            let code = [".py", ".xml", ".csv"]
                .iter()
                .any(|ext| inner.ends_with(ext));
            *out.entry(dir).or_default() |= code;
        }
    }
    out
}

/// The manifest file of `dir` at `rev`, if `dir` is a module there.
fn manifest_at(clone: &str, rev: &str, dir: &str) -> Option<&'static str> {
    MANIFESTS
        .into_iter()
        .find(|m| git_run(clone, &["cat-file", "-e", &format!("{rev}:{dir}/{m}")]).is_ok())
}

/// `dir` at `sha`, extracted under `dest`.
fn extract(clone: &str, sha: &str, dir: &str, dest: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(dest).map_err(|err| err.to_string())?;
    cmd!("git", "-c", "safe.directory=*", "archive", sha, dir)
        .dir(clone)
        .pipe(cmd!("tar", "-x", "-C", dest))
        .run()
        .map_err(|err| err.to_string())?;
    // `git archive` keeps symlinks and the analyzer follows them (open(),
    // readme reads): a PR could point one at a secret.
    cmd!("find", dest, "-type", "l", "-delete")
        .run()
        .map_err(|err| err.to_string())?;
    Ok(dest.join(dir))
}

/// (code, xml id, file) of the grave-or-not findings of the module this PR
/// changes as merged: on this branch, or for a port, its newest lower version
/// (any repo of the org: modules move). Findings outside it are "new".
fn base_findings(
    conn: &mut SqliteConnection,
    c: &RepoPrs,
    name: &str,
    is_new: bool,
) -> HashSet<(String, Option<String>, Option<String>)> {
    let version = c.version_odoo as i32;
    let base =
        models::module::get_by_technical_name_organization_name(conn, name, c.repo.get_org())
            .into_iter()
            .filter(|m| match is_new {
                true => m.version_odoo < version,
                false => m.version_odoo == version && m.gh_repository_id == c.gh_repo_id,
            })
            .max_by_key(|m| m.version_odoo);
    let Some(base) = base else {
        return HashSet::new();
    };
    let Some(snapshot) = models::module_version::get_by_module_id_version_module(
        conn,
        &base.id,
        &base.version_module,
    ) else {
        return HashSet::new();
    };
    models::module_security_warning::get_by_module_version_id(conn, &snapshot.id)
        .into_iter()
        .map(|w| (w.code, w.xml_id, w.file))
        .collect()
}

/// The analysis JSON of one module at the PR head, `None` (logged) when it
/// can't be read.
fn analyze_module(
    c: &RepoPrs,
    conn: &mut SqliteConnection,
    pr: &PullRequest,
    dest: &Path,
    dir: &str,
    manifest_file: &str,
    is_new: bool,
) -> Option<Value> {
    let sha = pr.head_sha.as_deref()?;
    let warn = |err: String| {
        log::warn!(
            "PR #{} of '{}': can't analyze '{dir}': {err}",
            pr.prid,
            c.repo.get_name()
        );
    };
    let path = extract(c.repo.get_clone_path(), sha, dir, dest)
        .map_err(warn)
        .ok()?;
    let manifest = analyzer::literal_manifest(&path.join(manifest_file))
        .map_err(warn)
        .ok()?;
    let name = dir.rsplit('/').next().unwrap_or(dir);
    let analysis = c.analyzer.analyze_module_source(&path);
    let category = manifest["category"].as_str().unwrap_or_default();
    let base = base_findings(conn, c, name, is_new);
    let security: Vec<Value> = security::analyze_module(name, category, &analysis)
        .into_iter()
        .map(|w| {
            let new = !base.contains(&(w.code.clone(), w.xml_id.clone(), w.file.clone()));
            let mut finding = json!(w);
            finding["new"] = json!(new);
            finding
        })
        .collect();
    Some(json!({
        "name": manifest["name"],
        "version": manifest["version"],
        "license": manifest["license"],
        "installable": manifest.get("installable").cloned().unwrap_or(json!(true)),
        "depends": manifest["depends"],
        "security": security,
        "migration_notes": migration::analyze_module(c.version_odoo, &analysis),
    }))
}

/// Grave findings an analysis marks as introduced by the PR.
fn new_grave_findings(analysis: &Value) -> Vec<(String, String)> {
    analysis["security"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|w| w["new"] == true && w["severity"] == "error")
        .map(|w| {
            let key = format!("{}|{}|{}", w["code"], w["xml_id"], w["file"]);
            (key, w["message"].as_str().unwrap_or_default().to_string())
        })
        .collect()
}

/// Maps a PR to the modules it changes and analyzes them. A fetch/diff
/// failure only warns (one bad PR must not fail the job) and leaves the PR
/// unanalyzed, so the next run retries it.
fn analyze(c: &RepoPrs, conn: &mut SqliteConnection, pr: &PullRequest) {
    let Some(sha) = pr.head_sha.as_deref() else {
        return;
    };
    let (clone, repo_name) = (c.repo.get_clone_path(), c.repo.get_name());
    if let Err(err) = c.git.fetch_pull_request(clone, c.repo.get_url(), pr.prid) {
        log::warn!("PR #{} of '{repo_name}': fetch failed: {err}", pr.prid);
        return;
    }
    let base = format!("origin/{}", c.branch);
    let range = format!("{base}...{sha}");
    let diff = match git_run(clone, &["diff", "--no-renames", "--name-only", &range]) {
        Ok(diff) => diff,
        Err(err) => {
            log::warn!("PR #{} of '{repo_name}': diff failed: {err}", pr.prid);
            return;
        }
    };
    log::info!("Analyzing PR #{} of '{repo_name}'...", pr.prid);
    let files: Vec<&str> = diff.lines().collect();
    let dest = c.workdir.join(format!("{repo_name}-{}", pr.prid));
    let mut rows: Vec<(String, bool, Option<String>)> = Vec::new();
    for (dir, code_changed) in touched_modules(&files, c.read_paths) {
        let Some(manifest_file) = manifest_at(clone, sha, &dir) else {
            continue; // not a module at the head (removed, or not one at all)
        };
        let is_new = manifest_at(clone, &base, &dir).is_none();
        let analyzed = rows.iter().filter(|(_, _, a)| a.is_some()).count();
        let analysis = (code_changed && analyzed < MAX_ANALYZED_MODULES)
            .then(|| analyze_module(c, conn, pr, &dest, &dir, manifest_file, is_new))
            .flatten();
        let name = dir.rsplit('/').next().unwrap_or(&dir).to_string();
        rows.push((name, is_new, analysis.map(|a| a.to_string())));
    }
    let _ = fs::remove_dir_all(&dest);

    let new_rows: Vec<NewPullRequestModule> = rows
        .iter()
        .map(|(name, is_new, analysis)| NewPullRequestModule {
            pull_request_id: pr.id,
            module_technical_name: name,
            is_new: *is_new,
            analysis: analysis.as_deref(),
        })
        .collect();
    let previous =
        models::pull_request_module::replace_for_pull_request(conn, pr.id, &new_rows).unwrap();
    models::pull_request::set_analyzed(conn, &pr.id, sha, analyzer::ANALYSIS_VERSION).unwrap();

    // A new head of a PR analyzed before, or a PR opened in the last day; not
    // a first look at an old PR, nor a re-analysis for new rules.
    let fresh = (pr.analyzed_sha.is_some() && pr.analyzed_sha != pr.head_sha)
        || models::pull_request::days_since(pr.created_at.as_deref()).is_some_and(|d| d <= 1);
    if !fresh {
        return;
    }
    for (name, is_new, analysis) in &rows {
        let before = previous.iter().find(|p| &p.module_technical_name == name);
        if *is_new && !before.is_some_and(|p| p.is_new) {
            let _ = system_event::register_new_migration_pr(
                conn,
                name,
                &pr.name,
                &pr.prid,
                repo_name,
                &c.version_odoo,
            );
        }
        let Some(analysis) = analysis
            .as_deref()
            .and_then(|a| serde_json::from_str(a).ok())
        else {
            continue;
        };
        let known: HashSet<String> = before
            .and_then(|p| p.analysis.as_deref())
            .and_then(|a| serde_json::from_str::<Value>(a).ok())
            .map(|a| new_grave_findings(&a).into_iter().map(|(k, _)| k).collect())
            .unwrap_or_default();
        for (key, message) in new_grave_findings(&analysis) {
            if !known.contains(&key) {
                let _ = system_event::register_pull_request_security(
                    conn,
                    &pr.name,
                    &pr.prid,
                    repo_name,
                    &c.version_odoo,
                    name,
                    &message,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_migration_module_name_matches_oca_convention() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-sale_commission"),
            Some("sale_commission".to_string())
        );
        assert_eq!(
            extract_migration_module_name("17.0-mig-account_analytic_tag_taxonomies"),
            Some("account_analytic_tag_taxonomies".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_rejects_non_migration_branches() {
        assert_eq!(extract_migration_module_name("16.0-fix-foo"), None);
        assert_eq!(extract_migration_module_name("master"), None);
        assert_eq!(extract_migration_module_name("16.0-mig-"), None);
        assert_eq!(extract_migration_module_name("mig-sale_commission"), None);
    }

    #[test]
    fn test_extract_migration_module_name_drops_discard_suffix() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre_modulo_algo-BAD"),
            Some("nombre_modulo_algo".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_normalizes_hyphenated_name() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre-modulo-algo"),
            Some("nombre_modulo_algo".to_string())
        );
    }

    #[test]
    fn test_extract_migration_module_name_ambiguous_hyphenated_with_discard() {
        assert_eq!(
            extract_migration_module_name("16.0-mig-nombre-modulo-algo-BAD"),
            Some("nombre_modulo_algo_BAD".to_string())
        );
    }

    #[test]
    fn changed_files_map_to_module_folders() {
        let odoo = ["/addons".to_string(), "/odoo/addons".to_string()];
        let files = [
            "addons/sale/models/sale.py",
            "addons/sale/i18n/es.po",
            "addons/stock/i18n/es.po",
            "odoo/addons/base/data/res_partner.csv",
            "odoo/tools/misc.py", // not under a read path's folder
            "addons/README.md",   // a file, not a folder, of the read path
            "setup.py",
        ];
        let touched = touched_modules(&files, &odoo);
        let expected = [
            ("addons/sale", true),
            ("addons/stock", false), // translations only: no analysis
            ("odoo/addons/base", true),
        ];
        assert_eq!(
            touched,
            expected
                .map(|(d, c)| (d.to_string(), c))
                .into_iter()
                .collect()
        );
        // A repo-root read path: any top folder is a candidate.
        let root = [String::new()];
        let touched = touched_modules(&["web_foo/views/x.xml", "setup/web_foo/setup.py"], &root);
        assert_eq!(touched.keys().collect::<Vec<_>>(), ["setup", "web_foo"]);
    }

    #[test]
    fn a_pr_is_mapped_to_its_modules_and_analyzed() {
        use crate::clients::gitlab::GitlabClient;
        let dir = std::env::temp_dir().join(format!("ogh-pr-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let origin = dir.join("origin");
        fs::create_dir_all(&origin).unwrap();
        let git = |cwd: &Path, args: &[&str]| -> String {
            let args = ["-c", "user.name=T", "-c", "user.email=t@t"]
                .iter()
                .chain(args);
            cmd("git", args.copied())
                .dir(cwd)
                .env("GIT_CONFIG_GLOBAL", "/dev/null") // no commit signing
                .stderr_null()
                .read()
                .unwrap()
        };
        let write = |path: PathBuf, content: &str| {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        };
        git(&origin, &["init", "-q", "-b", "18.0"]);
        write(
            origin.join("mod_a/__manifest__.py"),
            "{'name': 'A', 'version': '18.0.1.0.0'}",
        );
        write(origin.join("mod_b/__manifest__.py"), "{'name': 'B'}");
        write(origin.join("mod_b/i18n/es.po"), "msgid \"\"\n");
        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "-q", "-m", "base"]);
        git(&origin, &["checkout", "-q", "-b", "pr"]);
        write(
            origin.join("mod_a/__manifest__.py"),
            "{'name': 'A', 'version': '18.0.1.1.0'}",
        );
        write(origin.join("mod_b/i18n/es.po"), "msgid \"x\"\n");
        write(
            origin.join("mod_new/__manifest__.py"),
            "{'name': 'New', 'version': '18.0.1.0.0', 'depends': ['mod_a']}",
        );
        std::os::unix::fs::symlink("/etc/hostname", origin.join("mod_new/data.xml")).unwrap();
        git(&origin, &["add", "-A"]);
        git(&origin, &["commit", "-q", "-m", "pr"]);
        let sha = git(&origin, &["rev-parse", "HEAD"]);
        // Not a GitHub URL: GitLab's MR ref.
        git(
            &origin,
            &["update-ref", "refs/merge-requests/7/head", "HEAD"],
        );
        git(&origin, &["checkout", "-q", "18.0"]);
        let origin_url = origin.to_string_lossy().to_string();
        git(
            &dir,
            &["clone", "-q", "--branch", "18.0", &origin_url, "clone"],
        );

        let pool = sqlitedb::new_write_pool(dir.join("data.db").to_str().unwrap());
        let mut conn = pool.get().unwrap();
        sqlitedb::run_migrations(&mut conn).unwrap();
        let org = models::gh_organization::add(&mut conn, "org").unwrap();
        let gh_repo = models::gh_repository::add(&mut conn, &org.id, "repo").unwrap();
        let (_, pr) = models::pull_request::add(
            &mut conn,
            &NewPullRequest {
                name: "pr",
                version_odoo: 18,
                prid: 7,
                gh_repository_id: gh_repo.id,
                created_at: None,
                ci_status: None,
                last_message_at: None,
                head_sha: Some(&sha),
                author: None,
                is_draft: false,
                labels: None,
            },
        )
        .unwrap();
        let repo = RepoInfo {
            name: "repo".to_string(),
            org: "org".to_string(),
            clone_path: dir.join("clone").to_string_lossy().to_string(),
            full_path: "org/repo".to_string(),
            url: origin_url,
        };
        let client = AnyGitClient::Gitlab(GitlabClient::new("x", ""));
        let defs = crate::workflow::with_built_in(&Default::default());
        let engine = Engine::new(&[], &defs).unwrap();
        let (analyzer, pypi) = (
            OGHCollectorAnalyzer::new(&18),
            crate::pypi::PypiClient::new(),
        );
        let forge = crate::actions::Forge::with(true, "http://127.0.0.1:9", None);
        let workdir = dir.join("work");
        let deps = Deps::for_tests(&analyzer, &pypi, &forge, &workdir, &[]);
        let c = RepoPrs {
            git: &client,
            repo: &repo,
            gh_repo_id: gh_repo.id,
            branch: "18.0",
            version_odoo: 18,
            read_paths: &[String::new()],
            analyzer: &OGHCollectorAnalyzer::new(&18),
            workdir: &dir.join("work"),
            full: true,
            engine: &engine,
            deps: &deps,
        };
        analyze(&c, &mut conn, &pr);

        let rows =
            models::pull_request_module::get_by_pull_request_ids(&mut conn, &[pr.id]).unwrap();
        let row = |name: &str| {
            rows.iter()
                .find(|r| r.module_technical_name == name)
                .unwrap()
        };
        let analysis = |name: &str| -> Value {
            serde_json::from_str(row(name).analysis.as_deref().unwrap()).unwrap()
        };
        assert_eq!(rows.len(), 3);
        assert!(!row("mod_a").is_new);
        assert_eq!(analysis("mod_a")["version"], "18.0.1.1.0");
        assert!(row("mod_b").analysis.is_none()); // translations only
        assert!(row("mod_new").is_new);
        assert_eq!(analysis("mod_new")["depends"][0], "mod_a");
        let stored = models::pull_request::get_by_id(&mut conn, &pr.id).unwrap();
        assert_eq!(stored.analyzed_sha.as_deref(), Some(sha.as_str()));
        assert!(!dir.join("work/repo-7").exists());

        // Symlinks never reach the analyzer.
        let dest = dir.join("extract");
        let path = extract(&repo.clone_path, &sha, "mod_new", &dest).unwrap();
        assert!(path.join("__manifest__.py").exists());
        assert!(fs::symlink_metadata(path.join("data.xml")).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_introduced_grave_findings_are_reported() {
        let analysis = json!({"security": [
            {"severity": "error", "code": "SQL", "xml_id": null, "file": "a.py", "message": "m1", "new": true},
            {"severity": "error", "code": "EVAL", "xml_id": null, "file": "b.py", "message": "m2", "new": false},
            {"severity": "warning", "code": "ACL", "xml_id": "x", "file": null, "message": "m3", "new": true},
        ]});
        let found = new_grave_findings(&analysis);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1, "m1");
    }
}
