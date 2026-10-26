// Copyright Alexandre D. Díaz
//! Doodba projects, behind `projects_enabled`: the fleet (`/projects`) and one project
//! (`/project/{org}/{repo}/{version}`), as the collector's `doodba.rs` stored them. The fleet is a
//! listing route (`AreaPool`): template drift, Postgres, addons, patches carried in repos.yaml
//! (and how many of their PRs/MRs are merged or closed), advisories of the pip pins, credentials
//! in the project files, and lint hook revs against the newest rev the fleet runs for the same
//! Odoo version (the template's own revs hang off jinja branches per version, so the fleet is
//! the baseline). The project page keeps `web::Data<Pool>`, like module pages: its addons
//! resolve to modules of any workspace, with where they stand at the next indexed version (by
//! name or an OpenUpgrade rename, else their open migration PR).
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models::doodba_project::{self, Patch, Repo, Row};
use sqlitedb::{models, Pool};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::routes::doodba_tools::get_pending_modules;
use crate::utils::get_minijinja_context;
use crate::workspace::AreaPool;

fn version_str(version_odoo: i32) -> String {
    odoo_version_u8_to_string(&(version_odoo as u8))
}

/// `https://github.com/OCA/pylint-odoo` → `pylint-odoo`.
fn hook_name(repo: &str) -> &str {
    repo.trim_end_matches('/')
        .trim_end_matches(".git")
        .rsplit('/')
        .next()
        .unwrap_or(repo)
}

/// Sort key of a rev: its numbers (`v8.0.20` → [8, 0, 20]); a commit hash
/// sorts below any tag.
fn rev_key(rev: &str) -> Vec<u64> {
    if rev.len() >= 7 && rev.chars().all(|c| c.is_ascii_hexdigit()) {
        return Vec::new();
    }
    rev.split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect()
}

/// The newest rev of each hook the fleet runs, per Odoo version; among
/// revs that don't order (commit hashes), the most used one.
fn newest_revs(rows: &[Row]) -> HashMap<(String, i32), String> {
    let mut counts: HashMap<(String, i32, String), usize> = HashMap::new();
    for row in rows {
        for lint in row.linters() {
            *counts
                .entry((lint.repo, row.project.version_odoo, lint.rev))
                .or_default() += 1;
        }
    }
    let mut out: HashMap<(String, i32), (Vec<u64>, usize, String)> = HashMap::new();
    for ((repo, version_odoo, rev), count) in counts {
        let candidate = (rev_key(&rev), count, rev);
        let best = out.entry((repo, version_odoo)).or_insert(candidate.clone());
        if candidate > *best {
            *best = candidate;
        }
    }
    out.into_iter().map(|(k, (_, _, rev))| (k, rev)).collect()
}

/// Hooks of `row` behind the fleet's newest for its version: (hook, rev, newest).
fn lint_behind(
    row: &Row,
    newest: &HashMap<(String, i32), String>,
) -> Vec<(String, String, String)> {
    row.linters()
        .into_iter()
        .filter_map(|l| {
            let top = newest.get(&(l.repo.clone(), row.project.version_odoo))?;
            (*top != l.rev).then(|| (hook_name(&l.repo).to_string(), l.rev, top.clone()))
        })
        .collect()
}

fn answer(row: &Row, key: &str) -> Option<String> {
    serde_json::from_str::<Value>(&row.project.answers)
        .ok()?
        .get(key)?
        .as_str()
        .map(str::to_string)
}

fn day(date: Option<&str>) -> Option<String> {
    date.map(|d| d.chars().take(10).collect())
}

#[derive(Serialize)]
struct FleetRow {
    org_name: String,
    repository: String,
    version: String,
    template_commit: Option<String>,
    template_date: Option<String>,
    behind: Option<i32>,
    level: i32,
    postgres: Option<String>,
    addons: i64,
    patches: usize,
    patches_done: usize,
    advisories: usize,
    exposed: usize,
    lint_behind: Vec<(String, String, String)>,
    last_commit: Option<String>,
}

#[derive(Serialize)]
struct TemplateVersion {
    src: String,
    commit: String,
    date: Option<String>,
    behind: Option<i32>,
    projects: usize,
}

#[derive(Serialize)]
struct LintRev {
    hook: String,
    version: String,
    rev: String,
    projects: usize,
    newest: bool,
}

#[get("/projects")]
pub async fn route(
    pool: AreaPool,
    all: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    // The lint baseline is the whole fleet's, as on the project page.
    let (rows, newest) = web::block(move || {
        let rows = doodba_project::list(&mut pool.get().unwrap());
        let newest = newest_revs(&doodba_project::list(&mut all.get().unwrap()));
        (rows, newest)
    })
    .await?;
    let top_behind = rows
        .iter()
        .filter_map(|r| r.project.template_behind)
        .max()
        .unwrap_or(0);

    let mut fleet: Vec<FleetRow> = rows
        .iter()
        .map(|r| {
            let behind = r.project.template_behind;
            let summary = r.summary();
            FleetRow {
                org_name: r.org_name.clone(),
                repository: r.repository.clone(),
                version: version_str(r.project.version_odoo),
                template_commit: r.project.template_commit.clone(),
                template_date: day(r.project.template_commit_date.as_deref()),
                behind,
                // 0 = current; 1-4 against the fleet's furthest behind.
                level: behind
                    .filter(|b| *b > 0)
                    .map_or(0, |b| (b * 4 + top_behind - 1) / top_behind),
                postgres: answer(r, "postgres_version"),
                addons: r.addon_count,
                patches: summary.patches,
                patches_done: summary.patches_done,
                advisories: summary.advisories,
                exposed: summary.exposed_credentials,
                lint_behind: lint_behind(r, &newest),
                last_commit: day(Some(&r.project.last_commit_date)),
            }
        })
        .collect();
    // Most template work first; unknown drift last.
    fleet.sort_by_key(|f| std::cmp::Reverse(f.behind.unwrap_or(-1)));

    let mut templates: BTreeMap<(String, String), TemplateVersion> = BTreeMap::new();
    for r in &rows {
        let (Some(src), Some(commit)) = (&r.project.template_src, &r.project.template_commit)
        else {
            continue;
        };
        templates
            .entry((src.clone(), commit.clone()))
            .or_insert_with(|| TemplateVersion {
                src: src
                    .split_once("://")
                    .map_or(src.as_str(), |(_, s)| s)
                    .trim_end_matches(".git")
                    .to_string(),
                commit: commit.clone(),
                date: day(r.project.template_commit_date.as_deref()),
                behind: r.project.template_behind,
                projects: 0,
            })
            .projects += 1;
    }
    let mut templates: Vec<TemplateVersion> = templates.into_values().collect();
    templates.sort_by_key(|t| (t.behind.is_none(), t.behind, std::cmp::Reverse(t.projects)));

    let mut revs: BTreeMap<(String, i32, String), usize> = BTreeMap::new();
    for r in &rows {
        for l in r.linters() {
            *revs
                .entry((l.repo, r.project.version_odoo, l.rev))
                .or_default() += 1;
        }
    }
    let mut lint_revs: Vec<LintRev> = revs
        .into_iter()
        .map(|((repo, version_odoo, rev), projects)| LintRev {
            newest: newest.get(&(repo.clone(), version_odoo)) == Some(&rev),
            hook: hook_name(&repo).to_string(),
            version: version_str(version_odoo),
            rev,
            projects,
        })
        .collect();
    lint_revs.sort_by(|a, b| {
        (
            &a.hook,
            std::cmp::Reverse(rev_key(&a.version)),
            std::cmp::Reverse(rev_key(&a.rev)),
        )
            .cmp(&(
                &b.hook,
                std::cmp::Reverse(rev_key(&b.version)),
                std::cmp::Reverse(rev_key(&b.rev)),
            ))
    });

    tmpl_env.render(
        "pages/projects.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "projects",
                current => fleet.iter().filter(|f| f.behind == Some(0)).count(),
                unknown => fleet.iter().filter(|f| f.behind.is_none()).count(),
                patched => fleet.iter().filter(|f| f.patches > 0).count(),
                patches => fleet.iter().map(|f| f.patches).sum::<usize>(),
                patches_done => fleet.iter().map(|f| f.patches_done).sum::<usize>(),
                advisories => fleet.iter().map(|f| f.advisories).sum::<usize>(),
                advised => fleet.iter().filter(|f| f.advisories > 0).count(),
                exposed => fleet.iter().filter(|f| f.exposed > 0).count(),
                lint_outdated => fleet.iter().filter(|f| !f.lint_behind.is_empty()).count(),
                top_projects => templates.iter().map(|t| t.projects).max().unwrap_or(0),
                top_lint => lint_revs.iter().map(|l| l.projects).max().unwrap_or(0),
                fleet,
                templates,
                lint_revs,
            )
        ),
    )
}

#[derive(Serialize)]
struct PatchView {
    repo: String,
    remote: String,
    git_ref: String,
    url: Option<String>,
    state: Option<String>,
}

#[derive(Serialize)]
struct AddonView {
    repo: String,
    technical_name: String,
    org_name: Option<String>,
    in_next: bool,
    pr: Option<crate::routes::doodba_tools::MigrationPendingModuleInfo>,
}

/// Web link of a PR/MR merged as a patch, from its remote.
fn patch_url(remote_url: &str, patch: &Patch) -> Option<String> {
    Some(oghutils::git::pull_request_url(remote_url, patch.number()?))
}

#[get("/project/{org}/{repository}/{version}")]
pub async fn route_project(
    pool: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    path: web::Path<(String, String, String)>,
) -> Result<impl Responder> {
    let (org, repository, version) = path.into_inner();
    let version_odoo = crate::utils::odoo_version(&version)? as i32;
    let data = web::block(move || {
        let mut conn = pool.get().unwrap();
        let row = doodba_project::get(&mut conn, &org, &repository, version_odoo)?;
        let next = models::module::get_odoo_versions(&mut conn)
            .into_iter()
            .filter(|v| *v > version_odoo)
            .min();
        let addons =
            doodba_project::addons(&mut conn, row.project.gh_repository_id, version_odoo, next);
        let missing: Vec<String> = addons
            .iter()
            .filter(|a| a.in_next == 0 && a.technical_name != "*")
            .map(|a| a.technical_name.clone())
            .collect();
        let mut prs: HashMap<String, _> = next
            .map(|n| get_pending_modules(&mut conn, &missing, &(n as u8)))
            .unwrap_or_default()
            .into_iter()
            .map(|pr| (pr.technical_name.clone(), pr))
            .collect();
        let addons: Vec<AddonView> = addons
            .into_iter()
            .map(|a| AddonView {
                // A private addon's PR only counts in the project's own repository.
                pr: prs.remove(&a.technical_name).filter(|pr| {
                    a.repo != "private"
                        || (pr.organization == row.org_name && pr.repository_name == row.repository)
                }),
                in_next: a.in_next != 0,
                repo: a.repo,
                technical_name: a.technical_name,
                org_name: a.org_name,
            })
            .collect();
        let fleet = doodba_project::list(&mut conn);
        Some((row, next, addons, newest_revs(&fleet)))
    })
    .await?;
    let Some((row, next, addons, newest)) = data else {
        return Err(actix_web::error::ErrorNotFound("No such Doodba project"));
    };

    let repos: Vec<Repo> = row.repos();
    let patches: Vec<PatchView> = repos
        .iter()
        .flat_map(|r| {
            r.patches.iter().map(|p| PatchView {
                repo: r.path.clone(),
                url: r
                    .remotes
                    .get(&p.remote)
                    .and_then(|remote| patch_url(remote, p)),
                remote: p.remote.clone(),
                git_ref: p.git_ref.clone(),
                state: p.state.clone(),
            })
        })
        .collect();
    let lint_behind = lint_behind(&row, &newest);
    let linters: Vec<Value> = row
        .linters()
        .into_iter()
        .map(|l| {
            let newest = lint_behind
                .iter()
                .find(|(hook, rev, _)| hook == hook_name(&l.repo) && *rev == l.rev)
                .map(|(_, _, newest)| newest.clone());
            serde_json::json!({"hook": hook_name(&l.repo), "repo": l.repo, "rev": l.rev, "newest": newest})
        })
        .collect();
    let counted = addons.iter().filter(|a| a.technical_name != "*").count();
    // Readiness only over collected addons: nothing is known of the rest.
    let tracked: Vec<&AddonView> = addons
        .iter()
        .filter(|a| a.technical_name != "*" && a.org_name.is_some())
        .collect();
    let git = oghutils::git::clone_url(row.url.as_deref(), &row.org_name, &row.repository);
    // "<hash> <subject>" lines.
    let template_log: Vec<String> =
        serde_json::from_str(&row.project.template_log).unwrap_or_default();

    tmpl_env.render(
        "pages/project.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "project",
                meta_title => format!("{} {}", row.repository, version_str(version_odoo)),
                version => version_str(version_odoo),
                next => next.map(version_str),
                in_next => tracked.iter().filter(|a| a.in_next).count(),
                in_pr => tracked.iter().filter(|a| !a.in_next && a.pr.is_some()).count(),
                tracked => tracked.len(),
                addon_total => counted,
                template_date => day(row.project.template_commit_date.as_deref()),
                template_log => template_log
                    .iter()
                    .map(|l| l.split_once(' ').unwrap_or((l, "")))
                    .collect::<Vec<_>>(),
                answers => serde_json::from_str::<Value>(&row.project.answers).unwrap_or_default(),
                dependencies => serde_json::from_str::<BTreeMap<String, Vec<String>>>(&row.project.dependencies).unwrap_or_default(),
                last_commit => day(Some(&row.project.last_commit_date)),
                repo_url => oghutils::git::files_url(&git),
                patches_done => patches.iter().filter(|p| matches!(p.state.as_deref(), Some("merged" | "closed"))).count(),
                advisories => row.advisories(),
                // "<file> <host>"
                exposed => row.exposed().iter().map(|e| {
                    let (file, host) = e.split_once(' ').unwrap_or((e, ""));
                    (file.to_string(), host.to_string())
                }).collect::<Vec<_>>(),
                p => row,
                repos,
                patches,
                linters,
                addons,
            )
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revs_sort_by_their_numbers_and_patches_link_to_their_pr() {
        assert!(rev_key("v9.0.4") > rev_key("v8.0.20"));
        assert!(rev_key("v8.0.20") > rev_key("v8.0.3"));
        assert!(rev_key("v0.0.1") > rev_key("f9b919b9868143135a9c9cb03021089cabba8223"));
        assert_eq!(
            hook_name("https://github.com/OCA/pylint-odoo.git"),
            "pylint-odoo"
        );
        let patch = |git_ref: &str| Patch {
            remote: "origin".into(),
            git_ref: git_ref.into(),
            state: None,
        };
        assert_eq!(
            patch_url(
                "https://github.com/OCA/web.git",
                &patch("refs/pull/2779/head")
            )
            .as_deref(),
            Some("https://github.com/OCA/web/pull/2779")
        );
        assert_eq!(
            patch_url(
                "https://forge.example/g/p.git",
                &patch("refs/merge-requests/7/head")
            )
            .as_deref(),
            Some("https://forge.example/g/p/-/merge_requests/7")
        );
        assert_eq!(
            patch_url("https://github.com/OCA/web.git", &patch("16.0-mig")),
            None
        );
    }
}
