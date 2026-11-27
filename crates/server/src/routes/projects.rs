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
use cached::{proc_macro::cached, stores::TimedSizedCache};
use minijinja::context;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models::doodba_project::{self, newest_revs, rev_key, Patch, Repo, Row, SharedAddon};
use sqlitedb::{models, Pool};

use crate::config::SERVER_CONFIG;
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
    /// Next indexed Odoo version and how many of the collected addons are
    /// already there / have a migration PR (`ready_tracked` = collected ones).
    next: Option<String>,
    ready: usize,
    in_pr: usize,
    ready_tracked: usize,
    /// What the administrator should do here, worst first.
    issues: Vec<Issue>,
    score: i32,
    /// Space-separated problem tags, what the filter chips match.
    flags: String,
}

#[derive(Serialize, Clone)]
struct Issue {
    severity: &'static str,
    text: String,
}

/// One patch (PR/MR or branch) carried by projects, for the whole fleet.
#[derive(Serialize)]
struct FleetPatch {
    repo: String,
    git_ref: String,
    url: Option<String>,
    state: Option<String>,
    projects: Vec<ProjectRef>,
}

/// (organization, repository, Odoo version) of a project.
type ProjectRef = (String, String, String);

/// A repos.yaml entry that two projects of one Odoo version configure differently.
#[derive(Serialize)]
struct RepoDrift {
    version: String,
    repo: String,
    variants: Vec<(String, Vec<ProjectRef>)>,
}

/// An addon several projects enable.
#[derive(Serialize)]
struct SharedAddonView {
    repo: String,
    technical_name: String,
    version: String,
    projects: Vec<ProjectRef>,
}

/// One advisory with every project that pins the affected package.
#[derive(Serialize)]
struct FleetAdvisory {
    package: String,
    id: String,
    fixed_in: String,
    summary: String,
    projects: Vec<(String, String, String, String)>,
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

type FleetData = (
    Vec<Row>,
    HashMap<(String, i32), String>,
    Vec<(Option<i32>, usize, usize, usize)>,
    Vec<SharedAddon>,
);

/// One query per project for the next-version readiness (~0.25 s): cached per workspace.
#[cached(
    ty = "TimedSizedCache<String, FleetData>",
    create = r#"{ TimedSizedCache::with_size_and_lifespan_and_refresh(50, std::time::Duration::from_secs(*SERVER_CONFIG.get_cache_ttl()), false) }"#,
    convert = r#"{ pool.key().to_string() }"#
)]
fn fleet_data(pool: &AreaPool, all: &Pool) -> FleetData {
    // The lint baseline is the whole fleet's, as on the project page.
    let rows = doodba_project::list(&mut pool.get().unwrap());
    let mut conn = all.get().unwrap();
    let newest = newest_revs(&doodba_project::list(&mut conn));
    let versions = models::module::get_odoo_versions(&mut conn);
    let shared = doodba_project::all_addons(&mut pool.get().unwrap());
    // (next version, in next, with a migration PR, collected addons)
    let readiness: Vec<(Option<i32>, usize, usize, usize)> = rows
        .iter()
        .map(|r| {
            let p = &r.project;
            let next = versions
                .iter()
                .copied()
                .filter(|v| *v > p.version_odoo)
                .min();
            let Some(n) = next else {
                return (None, 0, 0, 0);
            };
            let addons =
                doodba_project::addons(&mut conn, p.gh_repository_id, p.version_odoo, next);
            let tracked: Vec<_> = addons
                .iter()
                .filter(|a| a.technical_name != "*" && a.org_name.is_some())
                .collect();
            let missing: Vec<String> = tracked
                .iter()
                .filter(|a| a.in_next == 0)
                .map(|a| a.technical_name.clone())
                .collect();
            let pending = get_pending_modules(&mut conn, &missing, &(n as u8));
            let in_pr = missing
                .iter()
                .filter(|m| pending.iter().any(|pr| &pr.technical_name == *m))
                .count();
            (next, tracked.len() - missing.len(), in_pr, tracked.len())
        })
        .collect();
    (rows, newest, readiness, shared)
}

#[get("/projects")]
pub async fn route(
    pool: AreaPool,
    all: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    let all = all.get_ref().clone();
    let (rows, newest, readiness, shared) = web::block(move || fleet_data(&pool, &all)).await?;
    let top_behind = rows
        .iter()
        .filter_map(|r| r.project.template_behind)
        .max()
        .unwrap_or(0);

    let mut fleet: Vec<FleetRow> = rows
        .iter()
        .zip(readiness)
        .map(|(r, (next, ready, in_pr, ready_tracked))| {
            let behind = r.project.template_behind;
            let summary = r.summary();
            let lint = lint_behind(r, &newest);
            let mut issues = Vec::new();
            let mut flags = Vec::new();
            let mut score = 0;
            let elsewhere = r.exposed_elsewhere().len();
            if elsewhere > 0 {
                issues.push(Issue {
                    severity: "danger",
                    text: format!(
                        "Credentials written outside repos.yaml in {elsewhere} place{}: move them to secrets",
                        if elsewhere == 1 { "" } else { "s" }
                    ),
                });
                flags.push("credentials");
                score += 100;
            } else if summary.exposed_credentials > 0 {
                issues.push(Issue {
                    severity: "info",
                    text: "Token in repos.yaml (usual for a shared read token)".to_string(),
                });
            }
            if summary.advisories > 0 {
                issues.push(Issue {
                    severity: "danger",
                    text: format!(
                        "{} PyPI advisories on pinned pip dependencies: bump the pins",
                        summary.advisories
                    ),
                });
                flags.push("advisories");
                score += 50 + summary.advisories.min(20) as i32;
            }
            if summary.patches_done > 0 {
                issues.push(Issue {
                    severity: "warn",
                    text: format!(
                        "{} patch{} merged or closed upstream: drop from repos.yaml",
                        summary.patches_done,
                        if summary.patches_done == 1 { "" } else { "es" }
                    ),
                });
                flags.push("patches-done");
                score += 30;
            }
            if let Some(b) = behind.filter(|b| *b > 0) {
                issues.push(Issue {
                    severity: "warn",
                    text: format!("{b} commits behind the template: copier update"),
                });
                flags.push("behind");
                score += 20;
            }
            if !lint.is_empty() {
                issues.push(Issue {
                    severity: "info",
                    text: format!(
                        "{} lint hook{} behind the fleet's newest",
                        lint.len(),
                        if lint.len() == 1 { "" } else { "s" }
                    ),
                });
                flags.push("lint");
                score += 10;
            }
            FleetRow {
                next: next.map(version_str),
                ready,
                in_pr,
                ready_tracked,
                issues,
                score,
                flags: flags.join(" "),
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
                exposed: r.exposed_elsewhere().len(),
                lint_behind: lint,
                last_commit: day(Some(&r.project.last_commit_date)),
            }
        })
        .collect();
    // Most template work first; unknown drift last.
    fleet.sort_by_key(|f| std::cmp::Reverse(f.behind.unwrap_or(-1)));

    let mut queue: Vec<&FleetRow> = fleet.iter().filter(|f| f.score > 0).collect();
    queue.sort_by_key(|f| std::cmp::Reverse(f.score));
    queue.truncate(10);

    let mut patch_map: BTreeMap<(String, String), FleetPatch> = BTreeMap::new();
    let mut adv_map: BTreeMap<(String, String), FleetAdvisory> = BTreeMap::new();
    for r in &rows {
        let who = |extra: String| {
            (
                r.org_name.clone(),
                r.repository.clone(),
                version_str(r.project.version_odoo),
                extra,
            )
        };
        for repo in r.repos() {
            for p in &repo.patches {
                let remote = repo.remotes.get(&p.remote);
                let key = (
                    remote
                        .map(|u| strip_credentials_url(u))
                        .unwrap_or_else(|| p.remote.clone()),
                    p.git_ref.clone(),
                );
                let e = patch_map.entry(key.clone()).or_insert_with(|| FleetPatch {
                    repo: format!(
                        "{} @ {}",
                        repo.path.rsplit('/').next().unwrap_or(&repo.path),
                        key.0
                    ),
                    git_ref: p.git_ref.clone(),
                    url: remote.and_then(|u| patch_url(u, p)),
                    state: p.state.clone(),
                    projects: Vec::new(),
                });
                // The state is per fetch; keep the most decisive one.
                if e.state.is_none() || p.is_done() {
                    e.state = p.state.clone();
                }
                let w = who(String::new());
                e.projects.push((w.0, w.1, w.2));
            }
        }
        for a in r.advisories() {
            let w = who(a.version.clone());
            adv_map
                .entry((a.package.clone(), a.id.clone()))
                .or_insert_with(|| FleetAdvisory {
                    package: a.package.clone(),
                    id: a.id.clone(),
                    fixed_in: a.fixed_in.clone(),
                    summary: a.summary.clone(),
                    projects: Vec::new(),
                })
                .projects
                .push(w);
        }
    }
    // Same repos.yaml entry, same Odoo version, different target or patches.
    let mut drift_map: BTreeMap<(i32, String), BTreeMap<String, Vec<ProjectRef>>> = BTreeMap::new();
    for r in &rows {
        for repo in r.repos() {
            let mut refs: Vec<&str> = repo.patches.iter().map(|p| p.git_ref.as_str()).collect();
            refs.sort();
            let sig = format!(
                "{}{}",
                repo.target.as_deref().unwrap_or("default branch"),
                if refs.is_empty() {
                    String::new()
                } else {
                    format!(" + {}", refs.join(", "))
                }
            );
            drift_map
                .entry((r.project.version_odoo, repo.path.clone()))
                .or_default()
                .entry(sig)
                .or_default()
                .push((
                    r.org_name.clone(),
                    r.repository.clone(),
                    version_str(r.project.version_odoo),
                ));
        }
    }
    let mut repo_drift: Vec<RepoDrift> = drift_map
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|((v, repo), variants)| {
            let mut variants: Vec<_> = variants.into_iter().collect();
            variants.sort_by_key(|(_, p)| std::cmp::Reverse(p.len()));
            RepoDrift {
                version: version_str(v),
                repo,
                variants,
            }
        })
        .collect();
    repo_drift.sort_by(|a, b| {
        (std::cmp::Reverse(rev_key(&a.version)), &a.repo)
            .cmp(&(std::cmp::Reverse(rev_key(&b.version)), &b.repo))
    });

    let mut shared_map: BTreeMap<(i32, String, String), Vec<ProjectRef>> = BTreeMap::new();
    for a in shared {
        shared_map
            .entry((a.version_odoo, a.repo, a.technical_name))
            .or_default()
            .push((a.org_name, a.repository, version_str(a.version_odoo)));
    }
    let mut shared_addons: Vec<SharedAddonView> = shared_map
        .into_iter()
        .filter(|(_, p)| p.len() > 1)
        .map(|((v, repo, technical_name), projects)| SharedAddonView {
            repo,
            technical_name,
            version: version_str(v),
            projects,
        })
        .collect();
    shared_addons.sort_by_key(|a| std::cmp::Reverse(a.projects.len()));
    shared_addons.truncate(50);

    let mut fleet_patches: Vec<FleetPatch> = patch_map.into_values().collect();
    fleet_patches.sort_by_key(|p| {
        (
            std::cmp::Reverse(matches!(p.state.as_deref(), Some("merged" | "closed"))),
            std::cmp::Reverse(p.projects.len()),
        )
    });
    let mut fleet_advisories: Vec<FleetAdvisory> = adv_map.into_values().collect();
    fleet_advisories.sort_by_key(|a| std::cmp::Reverse(a.projects.len()));

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
                by_version => {
                    let mut counts = std::collections::BTreeMap::new();
                    for f in &fleet {
                        counts.entry(rev_key(&f.version)).or_insert((f.version.clone(), 0)).1 += 1;
                    }
                    counts.into_values().rev().collect::<Vec<_>>()
                },
                top_projects => templates.iter().map(|t| t.projects).max().unwrap_or(0),
                top_lint => lint_revs.iter().map(|l| l.projects).max().unwrap_or(0),
                queue,
                repo_drift,
                shared_addons,
                fleet_patches,
                fleet_advisories,
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
    repo_version: Option<String>,
    in_next: bool,
    pr: Option<crate::routes::doodba_tools::MigrationPendingModuleInfo>,
}

/// A manifest version as Odoo stores it once installed (`odoo.modules.module.adapt_version`):
/// prefixed with the series unless it already is.
fn adapt_version(serie: &str, version: &str) -> String {
    if version == serie || version.starts_with(&format!("{serie}.")) {
        version.to_string()
    } else {
        format!("{serie}.{version}")
    }
}

/// A remote without the `user:token@` part, so it can be shown and grouped on.
fn strip_credentials_url(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => format!(
            "{scheme}://{}",
            rest.rsplit_once('@').map_or(rest, |(_, h)| h)
        ),
        None => url.to_string(),
    }
}

/// Web link of a PR/MR merged as a patch, from its remote.
fn patch_url(remote_url: &str, patch: &Patch) -> Option<String> {
    Some(oghutils::git::pull_request_url(remote_url, patch.number()?))
}

/// Technical names of a project's addons (no `*`), for the migration planner.
#[get("/project/{org}/{repository}/{version}/addons")]
pub async fn route_project_addons(
    pool: web::Data<Pool>,
    path: web::Path<(String, String, String)>,
) -> Result<impl Responder> {
    let (org, repository, version) = path.into_inner();
    let version_odoo = crate::utils::odoo_version(&version)? as i32;
    let names = web::block(move || {
        let mut conn = pool.get().unwrap();
        let row = doodba_project::get(&mut conn, &org, &repository, version_odoo)?;
        let mut names: Vec<String> =
            doodba_project::addons(&mut conn, row.project.gh_repository_id, version_odoo, None)
                .into_iter()
                .map(|a| a.technical_name)
                .filter(|n| n != "*")
                .collect();
        names.sort();
        names.dedup();
        Some(names)
    })
    .await?
    .ok_or_else(|| actix_web::error::ErrorNotFound("No such Doodba project"))?;
    Ok(web::Json(names))
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
        let mut targets: Vec<i32> = models::module::get_odoo_versions(&mut conn)
            .into_iter()
            .filter(|v| *v > version_odoo)
            .collect();
        targets.sort();
        let next = targets.first().copied();
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
                repo_version: a.repo_version,
                repo: a.repo,
                technical_name: a.technical_name,
                org_name: a.org_name,
            })
            .collect();
        let fleet = doodba_project::list(&mut conn);
        Some((row, next, targets, addons, newest_revs(&fleet)))
    })
    .await?;
    let Some((row, next, targets, addons, newest)) = data else {
        return Err(actix_web::error::ErrorNotFound("No such Doodba project"));
    };

    // Declared in addons.yaml by name, yet absent from what an instance really has installed.
    let instances = row.instances();
    let not_installed = |env: &Option<doodba_project::Instance>| -> Vec<String> {
        let Some(env) = env.as_ref().filter(|e| !e.modules.is_empty()) else {
            return Vec::new();
        };
        let installed: std::collections::HashSet<&str> =
            env.modules.iter().map(|m| m.name.as_str()).collect();
        let mut missing: Vec<String> = addons
            .iter()
            .filter(|a| a.technical_name != "*" && !installed.contains(a.technical_name.as_str()))
            .map(|a| a.technical_name.clone())
            .collect();
        missing.dedup();
        missing
    };
    let not_installed = (
        not_installed(&instances.prod),
        not_installed(&instances.demo),
    );
    // Installed at another version than the repository's manifest: (module, installed, repo).
    let serie = version_str(version_odoo);
    let version_drift = |env: &Option<doodba_project::Instance>| -> Vec<(String, String, String)> {
        let Some(env) = env.as_ref() else {
            return Vec::new();
        };
        env.modules
            .iter()
            .filter_map(|m| {
                let installed = m.version.as_deref()?;
                let repo = addons
                    .iter()
                    .find(|a| a.technical_name == m.name)?
                    .repo_version
                    .as_deref()?;
                let repo = adapt_version(&serie, repo);
                (installed != repo).then(|| (m.name.clone(), installed.to_string(), repo))
            })
            .collect()
    };
    let version_drift = [
        version_drift(&instances.prod),
        version_drift(&instances.demo),
    ];

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
                targets => targets.into_iter().map(version_str).collect::<Vec<_>>(),
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
                deploy => row.deploy(),
                instances,
                not_installed => [not_installed.0, not_installed.1],
                version_drift,
                // "<file> <host>"
                exposed => row.exposed_elsewhere().iter().map(|e| {
                    let (file, host) = e.split_once(' ').unwrap_or((e, ""));
                    (file.to_string(), host.to_string())
                }).collect::<Vec<_>>(),
                repos_token => row.exposed().len() > row.exposed_elsewhere().len(),
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
    fn manifest_versions_compare_as_odoo_installs_them() {
        assert_eq!(adapt_version("18.0", "1.2.0"), "18.0.1.2.0");
        assert_eq!(adapt_version("18.0", "18.0.1.2.0"), "18.0.1.2.0");
        assert_eq!(adapt_version("18.0", "18.0"), "18.0");
    }

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

fn csv_field(v: &str) -> String {
    // Quote, and neutralise spreadsheet formulas.
    let v = if v.starts_with(['=', '+', '-', '@']) {
        format!("'{v}")
    } else {
        v.to_string()
    };
    format!("\"{}\"", v.replace('"', "\"\""))
}

/// The fleet as CSV, one project per line, for spreadsheets and scripts.
#[get("/projects.csv")]
pub async fn route_csv(pool: AreaPool) -> Result<impl Responder> {
    let rows = web::block(move || doodba_project::list(&mut pool.get().unwrap())).await?;
    let mut out = String::from(
        "organization,repository,odoo_version,template_commit,template_behind,addons,patches,patches_done,advisories,exposed_credentials,last_commit_date\n",
    );
    for r in &rows {
        let s = r.summary();
        let line = [
            csv_field(&r.org_name),
            csv_field(&r.repository),
            csv_field(&version_str(r.project.version_odoo)),
            csv_field(r.project.template_commit.as_deref().unwrap_or("")),
            s.template_behind.map_or(String::new(), |b| b.to_string()),
            s.addons.to_string(),
            s.patches.to_string(),
            s.patches_done.to_string(),
            s.advisories.to_string(),
            s.exposed_credentials.to_string(),
            csv_field(&r.project.last_commit_date),
        ]
        .join(",");
        out.push_str(&line);
        out.push('\n');
    }
    Ok(actix_web::HttpResponse::Ok()
        .content_type("text/csv; charset=utf-8")
        .insert_header((
            "Content-Disposition",
            "attachment; filename=\"doodba-projects.csv\"",
        ))
        .body(out))
}

/// Cache warm-up (`warmup.rs`).
pub(crate) fn warm(pool: &AreaPool, all: &Pool) {
    fleet_data_prime_cache(pool, all);
}
