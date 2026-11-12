// Copyright Alexandre D. Díaz
//! `on: project` workflows: routine fixes to a Doodba project, one commit per step, one PR/MR at
//! the end. `doodba::collect` runs them over every project it just stored (`Engine::run_project`),
//! so conditions see what the page of that project shows (`project.missing_addons`,
//! `project.lint_updates`, ...).
//!
//! `FoundProject` = the project as the job left it; `ProjectCtx` = one workflow's view: a lazily
//! created git worktree (detached at `origin/<branch>` under the job's workdir, never the shared
//! checkout), the commits made so far (`changes`, which `project_pr`'s `body` can list) and the
//! branch `project_pr` chose. The fix nodes (`drop_missing_addons`, `update_linters`) only edit the
//! worktree and commit there, so even in dry-run the report shows the real diffstat; nothing leaves
//! the collector until `project_pr` (outward, `actions`; its `auto_merge: true` also sets "merge when
//! the pipeline passes" on the PR/MR). The worktree is
//! removed by `Engine::run_all` once the walk ends, however it ended: project workflows can't use
//! `after` (a clone of the context would share it).
//!
//! What counts as missing (`missing_addons`): an `addons.yaml` entry that isn't `*`, whose repo is
//! collected at this version but doesn't have the module (a `private` one: no folder under
//! `odoo/custom/src/private`). A repo we didn't collect says nothing, so it's never "missing", and
//! that isn't the project page's "Missing" column (not in the *next* version): removing those
//! would break the running deployment. Lint targets (`lint_updates`): the fleet's newest tag of a
//! hook for this version (`doodba_project::newest_revs`), only moving a tag forward (a frozen sha
//! is the project's policy, and the "newest" of shas is just the most used).
//!
//! The YAML files are edited as text (`remove_addons`, `bump_revs`), never re-serialized: that
//! would drop comments and rewrite whole files in every PR. What a flow-style list or an unusual
//! layout hides isn't touched (and not reported as removed). Commit messages go through
//! `strip_credentials`.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{json, Value};
use sqlitedb::models::doodba_project::{self, rev_key};
use sqlitedb::DbSqliteConnection as SqliteConnection;

use oghutils::version::odoo_version_u8_to_string;

use crate::actions::commit_author;
use crate::doodba::{self, Project};
use crate::gitclient::{git_run, RepoInfo};
use crate::workflow::Deps;

const ADDONS: &str = "odoo/custom/src/addons.yaml";
const PRE_COMMIT: &str = ".pre-commit-config.yaml";

/// A project the job stored, with what the fix nodes act on.
pub struct FoundProject<'a> {
    pub repo: &'a RepoInfo,
    pub version_odoo: u8,
    /// The job's branch.
    pub branch: &'a str,
    pub project: &'a Project,
    pub template_behind: Option<i32>,
    /// `(repo, addon)` of `addons.yaml` the collected repos no longer have.
    pub missing: Vec<(String, String)>,
    /// `(hook repo, rev, newest rev)`.
    pub lint_updates: Vec<(String, String, String)>,
    /// The checkout's head: the ledger's item.
    pub head: String,
}

impl<'a> FoundProject<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conn: &mut SqliteConnection,
        repo: &'a RepoInfo,
        gh_repo_id: i64,
        version_odoo: u8,
        branch: &'a str,
        project: &'a Project,
        template_behind: Option<i32>,
        newest: &std::collections::HashMap<(String, i32), String>,
    ) -> FoundProject<'a> {
        let root = Path::new(repo.get_clone_path());
        let missing = doodba_project::addons(conn, gh_repo_id, version_odoo as i32, None)
            .into_iter()
            .filter(|a| a.org_name.is_none() && a.technical_name != "*")
            .filter(|a| match a.repo.as_str() {
                "private" => !root
                    .join("odoo/custom/src/private")
                    .join(&a.technical_name)
                    .exists(),
                name => doodba_project::repo_has_modules(conn, name, version_odoo as i32),
            })
            .map(|a| (a.repo, a.technical_name))
            .collect();
        let lint_updates = project
            .linters
            .iter()
            .filter_map(|l| {
                let top = newest.get(&(l.repo.clone(), version_odoo as i32))?;
                let (now, to) = (rev_key(&l.rev), rev_key(top));
                (!now.is_empty() && to > now).then(|| (l.repo.clone(), l.rev.clone(), top.clone()))
            })
            .collect();
        FoundProject {
            repo,
            version_odoo,
            branch,
            project,
            template_behind,
            missing,
            lint_updates,
            head: git_run(repo.get_clone_path(), &["rev-parse", "HEAD"])
                .unwrap_or_default()
                .trim()
                .to_string(),
        }
    }
}

/// One commit a fix node made.
#[derive(Clone, Serialize)]
struct Change {
    step: String,
    message: String,
    files: Vec<String>,
}

/// One workflow's view of a project.
#[derive(Clone)]
pub struct ProjectCtx<'f, 'a> {
    pub(crate) found: &'f FoundProject<'a>,
    pub(crate) ai: BTreeMap<String, String>,
    worktree: Option<PathBuf>,
    changes: Vec<Change>,
}

impl<'f, 'a> ProjectCtx<'f, 'a> {
    pub fn new(found: &'f FoundProject<'a>) -> Self {
        ProjectCtx {
            found,
            ai: BTreeMap::new(),
            worktree: None,
            changes: Vec::new(),
        }
    }

    pub(crate) fn json(&self) -> Value {
        let f = self.found;
        let p = f.project;
        let pairs = |list: &[(String, String)]| -> Vec<Value> {
            list.iter()
                .map(|(repo, name)| json!({"repo": repo, "name": name}))
                .collect()
        };
        json!({
            "org": f.repo.get_org(),
            "repo": f.repo.get_name(),
            "odoo_version": odoo_version_u8_to_string(&f.version_odoo),
            "project": {
                "answers": p.answers,
                "base_image": p.base_image,
                "template_behind": f.template_behind,
                "addons": pairs(&p.addons),
                "repos": p.repos,
                "linters": p.linters,
                "exposed": p.exposed,
                "missing_addons": pairs(&f.missing),
                "lint_updates": f.lint_updates.iter()
                    .map(|(repo, rev, newest)| json!({"repo": repo, "rev": rev, "newest": newest}))
                    .collect::<Vec<_>>(),
            },
            "changes": self.changes,
        })
    }

    /// The ledger's item.
    pub(crate) fn item(&self) -> String {
        let f = self.found;
        format!(
            "project:{}/{}@{}#{}",
            f.repo.get_org(),
            f.repo.get_name(),
            odoo_version_u8_to_string(&f.version_odoo),
            f.head
        )
    }

    pub(crate) fn label(&self) -> String {
        let f = self.found;
        format!(
            "project '{}' [{}]",
            f.repo.get_name(),
            odoo_version_u8_to_string(&f.version_odoo)
        )
    }

    /// `file("rel/path")`'s directory: the project's checkout.
    pub(crate) fn root(&self) -> String {
        self.found.repo.get_clone_path().to_string()
    }

    pub(crate) fn has_commits(&self) -> bool {
        !self.changes.is_empty()
    }

    pub(crate) fn worktree_path(&self) -> Option<&Path> {
        self.worktree.as_deref()
    }

    /// The commits, as `project_pr` reports them (dry-run too).
    pub(crate) fn summary(&self) -> String {
        let wt = self.worktree.as_deref().map(|p| p.to_string_lossy());
        let stat = wt
            .and_then(|wt| {
                git_run(
                    &wt,
                    &["diff", "--stat", &format!("origin/{}", self.found.branch)],
                )
                .ok()
            })
            .unwrap_or_default();
        let list: Vec<String> = self
            .changes
            .iter()
            .map(|c| format!("- {}: {}", c.step, c.message))
            .collect();
        format!("{}\n{stat}", list.join("\n"))
    }

    /// `ogh/<workflow>/<version>-<tree>`: the same fixes on the same files give
    /// the same branch, so a retry finds its earlier push.
    pub(crate) fn branch_name(&self, workflow: &str) -> Result<String, String> {
        let wt = self.worktree.as_deref().ok_or("nothing was changed")?;
        let tree = git_run(&wt.to_string_lossy(), &["rev-parse", "HEAD^{tree}"])?;
        let branch = format!(
            "ogh/{workflow}/{}-{}",
            odoo_version_u8_to_string(&self.found.version_odoo),
            &tree.trim()[..7.min(tree.trim().len())]
        );
        git_run(
            self.found.repo.get_clone_path(),
            &["check-ref-format", "--branch", &branch],
        )
        .map_err(|err| format!("bad branch '{branch}': {err}"))?;
        Ok(branch)
    }

    fn worktree(&mut self, deps: &Deps, workflow: &str) -> Result<PathBuf, String> {
        if let Some(wt) = &self.worktree {
            return Ok(wt.clone());
        }
        let f = self.found;
        let clone = f.repo.get_clone_path();
        let _ = git_run(clone, &["worktree", "prune"]);
        fs::create_dir_all(deps.workdir).map_err(|e| e.to_string())?;
        let name = format!(
            "project-{}-{}-{}-{workflow}",
            f.repo.get_org(),
            f.repo.get_name(),
            odoo_version_u8_to_string(&f.version_odoo)
        )
        .replace('/', "-");
        let wt = deps.workdir.join(name);
        let _ = fs::remove_dir_all(&wt);
        git_run(
            clone,
            &[
                "worktree",
                "add",
                "--detach",
                &wt.to_string_lossy(),
                &format!("origin/{}", f.branch),
            ],
        )?;
        self.worktree = Some(wt.clone());
        Ok(wt)
    }

    /// Drops the worktree, whatever happened to the walk.
    pub(crate) fn cleanup(&mut self) {
        if let Some(wt) = self.worktree.take() {
            let _ = git_run(
                self.found.repo.get_clone_path(),
                &["worktree", "remove", "--force", &wt.to_string_lossy()],
            );
            let _ = fs::remove_dir_all(&wt);
        }
    }

    /// A fix node: edits the worktree and commits (`message`, else a default); nothing to
    /// fix, nothing done. Returns whether it committed.
    pub(crate) fn fix(
        &mut self,
        kind: &str,
        step: &str,
        message: Option<String>,
        workflow: &str,
        deps: &Deps,
    ) -> Result<bool, String> {
        let f = self.found;
        let (file, default) = match kind {
            "drop_missing_addons" if !f.missing.is_empty() => (
                ADDONS,
                "fix(doodba): drop addons that no longer exist".to_string(),
            ),
            "update_linters" if !f.lint_updates.is_empty() => (
                PRE_COMMIT,
                "chore(pre-commit): update hooks to the fleet's newest".to_string(),
            ),
            _ => return Ok(false),
        };
        let wt = self.worktree(deps, workflow)?;
        let path = wt.join(file);
        let text = fs::read_to_string(&path).map_err(|e| format!("can't read {file}: {e}"))?;
        let (edited, done) = match kind {
            "drop_missing_addons" => {
                let (edited, gone) = remove_addons(&text, &f.missing);
                let list: Vec<String> = gone.iter().map(|(r, n)| format!("{r}/{n}")).collect();
                (edited, list)
            }
            _ => {
                let (edited, moved) = bump_revs(&text, &f.lint_updates);
                let list: Vec<String> = moved
                    .iter()
                    .map(|(r, from, to)| format!("{} {from} -> {to}", doodba_hook(r)))
                    .collect();
                (edited, list)
            }
        };
        if done.is_empty() {
            return Ok(false);
        }
        fs::write(&path, edited).map_err(|e| e.to_string())?;
        let message = format!(
            "{}\n\n{}",
            message.unwrap_or(default).trim(),
            done.iter()
                .map(|d| format!("- {d}"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let message = doodba::strip_credentials(&message);
        let files = commit(&wt, &message)?;
        self.changes.push(Change {
            step: step.to_string(),
            message: message.lines().next().unwrap_or_default().to_string(),
            files,
        });
        Ok(true)
    }
}

impl ProjectCtx<'_, '_> {
    /// The `pre_commit` node: runs pre-commit over the worktree (after the earlier steps) and
    /// commits what its hooks fixed. A hook that fixes a file exits 1, so pass 1 failing is
    /// normal: with changes they are committed and pass 2 must exit 0; failing with no changes is
    /// a real failure (no PR is opened, the error ends this item's workflow). Exit 0 at once,
    /// no `.pre-commit-config.yaml` or no earlier commit: nothing to do. Returns whether it committed.
    /// Hooks are foreign code (the project's `local` hooks, hook repos fetched from the network):
    /// the environment is scrubbed (no forge tokens), a timeout bounds each pass.
    pub(crate) fn pre_commit(
        &mut self,
        step: &str,
        message: Option<String>,
        deps: &Deps,
    ) -> Result<bool, String> {
        let Some(wt) = self.worktree.clone() else {
            return Ok(false);
        };
        if !wt.join(PRE_COMMIT).is_file() {
            return Ok(false);
        }
        let dir = wt.to_string_lossy().to_string();
        let home = std::env::var_os("OGHCOLLECTOR_PRE_COMMIT_HOME")
            .map_or_else(|| deps.workdir.join(".pre-commit"), PathBuf::from);
        let Some(first) = run_pre_commit(&wt, &home)? else {
            return Ok(false);
        };
        if git_run(&dir, &["status", "--porcelain"])?.trim().is_empty() {
            return Err(format!("pre-commit fails and fixes nothing:\n{first}"));
        }
        let message = message.unwrap_or_else(|| "style: pre-commit fixes".to_string());
        let message = doodba::strip_credentials(&message);
        let files = commit(&wt, &message)?;
        if let Some(out) = run_pre_commit(&wt, &home)? {
            return Err(format!(
                "pre-commit still fails after its own fixes:\n{out}"
            ));
        }
        self.changes.push(Change {
            step: step.to_string(),
            message: message.lines().next().unwrap_or_default().to_string(),
            files,
        });
        Ok(true)
    }
}

/// One `pre-commit run --all-files` in `wt`: `None` = exit 0, else the tail of its output.
fn run_pre_commit(wt: &Path, home: &Path) -> Result<Option<String>, String> {
    let out = std::process::Command::new("timeout")
        .args([
            "600",
            "pre-commit",
            "run",
            "--all-files",
            "--show-diff-on-failure",
        ])
        .current_dir(wt)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("PRE_COMMIT_HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LANG", "C.UTF-8")
        .output()
        .map_err(|e| format!("can't run pre-commit: {e}"))?;
    if out.status.success() {
        return Ok(None);
    }
    if out.status.code() == Some(124) {
        return Err("pre-commit timed out".to_string());
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(15)..].join("\n");
    Ok(Some(doodba::strip_credentials(&tail)))
}

/// The name of a hook repo, for messages.
fn doodba_hook(repo: &str) -> &str {
    repo.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(repo)
}

/// Commits what's staged-able in the worktree; the files it changed (none: no commit).
fn commit(wt: &Path, message: &str) -> Result<Vec<String>, String> {
    let dir = wt.to_string_lossy();
    git_run(&dir, &["add", "-A"])?;
    if git_run(&dir, &["diff", "--cached", "--quiet"]).is_ok() {
        return Ok(Vec::new());
    }
    let (name, email) = commit_author();
    git_run(
        &dir,
        &[
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            message,
        ],
    )?;
    let files = git_run(&dir, &["show", "--name-only", "--format=", "HEAD"])?;
    Ok(files.lines().map(str::to_string).collect())
}

static YAML_ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^\s*-\s+(?:"([^"]+)"|'([^']+)'|([^\s#'"]+))\s*(?:#.*)?$"#).unwrap()
});
static YAML_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(?:"([^"]+)"|'([^']+)'|([^\s#'"\-][^:]*)):\s*(?:#.*)?$"#).unwrap()
});
static PRE_COMMIT_REPO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*(?:-\s+)?repo:\s*["']?([^\s"'#]+)"#).unwrap());
static PRE_COMMIT_REV: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(\s*(?:-\s+)?rev:\s*["']?)([^\s"'#]+)(["']?.*)$"#).unwrap());

fn capture(caps: &regex::Captures) -> String {
    (1..caps.len())
        .find_map(|i| caps.get(i))
        .map(|m| m.as_str().to_string())
        .unwrap_or_default()
}

/// `addons.yaml` without the `remove` entries `(repo, addon)` that are list items under their
/// repo (any document of it), and which of them it removed. A repo left with no items loses
/// its key too (an empty one reads as null). Comments, order and quoting of the rest stay.
pub(crate) fn remove_addons(
    text: &str,
    remove: &[(String, String)],
) -> (String, Vec<(String, String)>) {
    let mut kept: Vec<&str> = Vec::new();
    let mut gone = Vec::new();
    // The current repo: its key line's place in `kept`, items kept, items removed.
    let mut key: Option<(String, usize, usize, usize)> = None;
    let mut empty_keys: Vec<usize> = Vec::new();
    let mut close = |key: &mut Option<(String, usize, usize, usize)>| {
        if let Some((_, at, items, removed)) = key.take() {
            if removed > 0 && items == 0 {
                empty_keys.push(at);
            }
        }
    };
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end();
        if bare == "---" || bare.starts_with("--- ") {
            close(&mut key);
        } else if !line.starts_with([' ', '\t', '#', '-', '\n', '\r']) && !bare.is_empty() {
            close(&mut key);
            if let Some(caps) = YAML_KEY.captures(bare) {
                key = Some((capture(&caps), kept.len(), 0, 0));
            }
        } else if let (Some(caps), Some(k)) = (YAML_ITEM.captures(bare), key.as_mut()) {
            let name = capture(&caps);
            if remove.iter().any(|(r, n)| *r == k.0 && *n == name) {
                gone.push((k.0.clone(), name));
                k.3 += 1;
                continue;
            }
            k.2 += 1;
        }
        kept.push(line);
    }
    close(&mut key);
    let out = kept
        .iter()
        .enumerate()
        .filter(|(i, _)| !empty_keys.contains(i))
        .map(|(_, l)| *l)
        .collect();
    (out, gone)
}

/// `.pre-commit-config.yaml` with each `(hook repo, from, to)` rev moved, and which moved:
/// only a `rev:` equal to `from` under that `repo:` changes, the rest of its line stays.
pub(crate) fn bump_revs(
    text: &str,
    updates: &[(String, String, String)],
) -> (String, Vec<(String, String, String)>) {
    let mut out = String::with_capacity(text.len());
    let mut moved = Vec::new();
    let mut repo: Option<String> = None;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end();
        if let Some(caps) = PRE_COMMIT_REPO.captures(bare) {
            repo = Some(doodba::clean_url(&caps[1]));
        } else if let Some(caps) = PRE_COMMIT_REV.captures(bare) {
            let hit = updates
                .iter()
                .find(|(r, from, _)| Some(r) == repo.as_ref() && *from == caps[2]);
            if let Some(update) = hit {
                let ending = &line[bare.len()..];
                out.push_str(&format!("{}{}{}{ending}", &caps[1], update.2, &caps[3]));
                moved.push(update.clone());
                continue;
            }
        }
        out.push_str(line);
    }
    (out, moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(repo: &str, name: &str) -> (String, String) {
        (repo.to_string(), name.to_string())
    }

    #[test]
    fn addons_are_removed_as_text_keeping_everything_else() {
        let text = "---\nENV:\n  X: a\n---\n# tools\nserver-tools:\n  - auditlog  # keep\n  - \"gone_a\"\n  - gone_b\nweb:\n- gone_c\nprivate:\n  - '*'\nother:\n  - gone_a\n";
        let remove = [
            pair("server-tools", "gone_a"),
            pair("server-tools", "gone_b"),
            pair("web", "gone_c"),
            pair("nothere", "x"),
        ];
        let (out, gone) = remove_addons(text, &remove);
        assert_eq!(
            gone,
            [
                pair("server-tools", "gone_a"),
                pair("server-tools", "gone_b"),
                pair("web", "gone_c")
            ]
        );
        // `web` is left with no items: its key goes too. `other/gone_a` isn't asked for.
        assert_eq!(
            out,
            "---\nENV:\n  X: a\n---\n# tools\nserver-tools:\n  - auditlog  # keep\nprivate:\n  - '*'\nother:\n  - gone_a\n"
        );
    }

    #[test]
    fn revs_move_only_where_they_match() {
        let text = "repos:\n  - repo: https://github.com/OCA/pre-commit-hooks\n    rev: v1.0.0  # tag\n    hooks: []\n  - repo: \"https://user:tok@github.com/OCA/other\"\n    rev: \"v2.0.0\"\n  - repo: https://github.com/x/y\n    rev: v1.0.0\n";
        let updates = [
            (
                "https://github.com/OCA/pre-commit-hooks".to_string(),
                "v1.0.0".to_string(),
                "v1.2.0".to_string(),
            ),
            (
                "https://github.com/OCA/other".to_string(),
                "v2.0.0".to_string(),
                "v2.1.0".to_string(),
            ),
        ];
        let (out, moved) = bump_revs(text, &updates);
        assert_eq!(moved.len(), 2);
        assert!(out.contains("rev: v1.2.0  # tag\n"), "{out}");
        assert!(out.contains("rev: \"v2.1.0\"\n"), "{out}");
        // Same rev under another repo: untouched.
        assert!(out.ends_with("repo: https://github.com/x/y\n    rev: v1.0.0\n"));
    }
}
