// Copyright Alexandre D. Díaz
//! Version migration (`/migration?from=17.0&to=18.0`): where the installable
//! modules of one Odoo version stand in another (migrated, renamed or merged
//! upstream, in an open migration PR, not started, or new there, and of
//! those, the ones no earlier indexed version has: `first`), per
//! repository (its module count at every version) and per module (the
//! versions it exists at). Same name = same module unless OpenUpgrade
//! records a rename/merge on the way (`openupgrade_rename`, followed through
//! every indexed version in `(from, to]`), and `is_new` PRs, as the Overview,
//! which takes its "In X, not in Y" list from here so both pages agree.
//! Listing route: `AreaPool` picks which modules are listed (the
//! workspace's), but their status comes from every workspace (a module
//! ported, renamed into or PR'd from another workspace's repository is
//! still ported, renamed or in a PR; see `sqlitedb::models::module`), and
//! PR links resolve through the global pool. Cache keyed on `pool.key()`.
//! Modules migrated under their own name are counted, never listed:
//! thousands of rows nobody has to act on.
use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};

use actix_web::{get, web, HttpRequest, Responder, Result};
use cached::{proc_macro::cached, stores::TimedSizedCache};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::config::SERVER_CONFIG;
use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::routes::doodba_tools::{get_pending_modules, MigrationPendingModuleInfo};
use crate::routes::module::pick_compare_versions;
use crate::utils::get_minijinja_context;
use crate::workspace::AreaPool;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

fn version_str(version_odoo: i32) -> String {
    odoo_version_u8_to_string(&(version_odoo as u8))
}

fn pct(part: i64, total: i64) -> i64 {
    if total > 0 {
        part * 100 / total
    } else {
        0
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct MigrationModule {
    pub technical_name: String,
    pub org_name: String,
    pub repository: String,
    /// `renamed` (or merged) | `pr` | `not_started` | `new`.
    pub status: &'static str,
    pub dependents: i64,
    /// `new` and at no indexed version before `to`, nor what OpenUpgrade
    /// renamed or merged into it.
    pub first: bool,
    /// Per indexed version (ascending): 2 installable, 3 lives on renamed or
    /// merged, 1 open migration PR, 0 neither.
    pub cells: Vec<u8>,
    /// The same, in words: "8.0–16.0, 18.0 · PR 19.0".
    pub life: String,
    pub pr: Option<MigrationPendingModuleInfo>,
    pub upstream: Option<Upstream>,
}

/// What OpenUpgrade records about a module between the two versions.
#[derive(Debug, Serialize, Clone)]
pub struct Upstream {
    /// `renamed` / `merged`: it became `name` (a pending module whose new
    /// name isn't at `to` yet keeps its status). `replaces`: a new module
    /// that `name` (one or more, comma separated) became.
    pub kind: &'static str,
    pub name: String,
    pub version: String,
    /// `name`'s organization, when it's installable at `to` (its page link).
    pub org_name: Option<String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct Tally {
    pub from_modules: i64,
    pub ported: i64,
    pub renamed: i64,
    pub pr: i64,
    pub not_started: i64,
    pub new: i64,
    /// Of `new`, those `first` there.
    pub first: i64,
    /// Migrated under its name or another: `ported` + `renamed`.
    pub ported_pct: i64,
    pub pr_pct: i64,
}

impl Tally {
    fn add(&mut self, status: &str, first: bool) {
        self.first += first as i64;
        match status {
            "new" => self.new += 1,
            "ported" => self.ported += 1,
            "renamed" => self.renamed += 1,
            "pr" => self.pr += 1,
            _ => self.not_started += 1,
        }
        if status != "new" {
            self.from_modules += 1;
        }
    }

    fn finish(mut self) -> Self {
        self.ported_pct = pct(self.ported + self.renamed, self.from_modules);
        self.pr_pct = pct(self.pr, self.from_modules);
        self
    }
}

#[derive(Debug, Serialize, Clone)]
pub struct RepoCell {
    pub modules: i64,
    /// 0-4, against the repository's busiest version.
    pub level: i64,
}

#[derive(Debug, Serialize, Clone)]
pub struct MigrationRepo {
    pub org_name: String,
    pub repository: String,
    pub cells: Vec<RepoCell>,
    pub tally: Tally,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct MigrationData {
    /// Every indexed version, ascending.
    pub versions: Vec<String>,
    pub from: String,
    pub to: String,
    /// Their positions in `versions`, to mark them in the per-module strips.
    pub from_idx: usize,
    pub to_idx: usize,
    pub tally: Tally,
    /// All but `ported`, most depended-on first.
    pub modules: Vec<MigrationModule>,
    /// Repositories with modules at `from` or `to`, most pending work first.
    pub repos: Vec<MigrationRepo>,
}

fn parse_versions(list: Option<&str>) -> HashSet<i32> {
    list.unwrap_or_default()
        .split(',')
        .filter_map(|v| v.parse().ok())
        .collect()
}

// Runs of consecutive indexed versions: "8.0–16.0, 18.0".
fn runs(versions: &[i32], keep: impl Fn(i32) -> bool) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut start: Option<i32> = None;
    for (i, v) in versions.iter().enumerate() {
        if !keep(*v) {
            continue;
        }
        start.get_or_insert(*v);
        if versions.get(i + 1).is_none_or(|next| !keep(*next)) {
            let first = start.take().unwrap();
            out.push(match first == *v {
                true => version_str(*v),
                false => format!("{}–{}", version_str(first), version_str(*v)),
            });
        }
    }
    out.join(", ")
}

/// `versions_desc`: every indexed version, newest first (`get_odoo_versions`).
#[cached(
    ty = "TimedSizedCache<(String, i32, i32), MigrationData>",
    key = "(String, i32, i32)",
    create = r#"
        {
            let ttl_secs = *SERVER_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(32, std::time::Duration::from_secs(ttl_secs), true)
        }
    "#,
    convert = r#"{ (_workspace.to_string(), from, to) }"#
)]
pub fn compute_migration_data(
    conn: &mut SqliteConnection,
    all: &mut SqliteConnection, // every workspace: PR repositories
    _workspace: &str,           // cache key only
    versions_desc: &[i32],
    from: i32,
    to: i32,
) -> MigrationData {
    let versions: Vec<i32> = versions_desc.iter().rev().copied().collect();
    let rows = models::module::version_transition(conn, from, to);
    let lives: Vec<HashSet<i32>> = rows
        .iter()
        .map(|m| parse_versions(Some(&m.versions)))
        .collect();
    // In any workspace: where a rename lands needn't be in this one.
    let at_to: HashMap<String, String> = models::module::installable_everywhere(conn, to)
        .into_iter()
        .map(|m| (m.technical_name, m.org_name))
        .collect();
    let renames: HashMap<(i32, String), (String, bool)> = models::openupgrade_rename::all(conn)
        .into_iter()
        .map(|r| ((r.version_odoo, r.old_name), (r.new_name, r.merged)))
        .collect();
    // Where a `from` module ended up at `to`, if OpenUpgrade renamed or
    // merged it on the way: (name, merged at some step, version of the last
    // step). Forward pairs only.
    let steps: Vec<i32> = versions
        .iter()
        .copied()
        .filter(|v| from < *v && *v <= to)
        .collect();
    let follow = |name: &str| {
        let mut end: Option<(String, bool, i32)> = None;
        for v in &steps {
            let cur = end.as_ref().map_or(name, |(n, ..)| n.as_str());
            if let Some((new_name, merged)) = renames.get(&(*v, cur.to_string())) {
                let merged = *merged || end.as_ref().is_some_and(|(_, m, _)| *m);
                end = Some((new_name.clone(), merged, *v));
            }
        }
        end
    };
    let ends: HashMap<&str, (String, bool, i32)> = rows
        .iter()
        .zip(&lives)
        .filter(|(m, present)| m.scope_from && !present.contains(&to))
        .filter_map(|(m, _)| Some((m.technical_name.as_str(), follow(&m.technical_name)?)))
        .collect();
    // What each new module took over, one step at a time.
    let mut replaces: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for ((v, old), (new_name, _)) in &renames {
        if steps.contains(v) {
            replaces.entry(new_name.as_str()).or_default().push(old);
        }
    }
    replaces.values_mut().for_each(|olds| olds.sort());

    let mut tally = Tally::default();
    let mut repo_tally: HashMap<(String, String), Tally> = HashMap::new();
    let mut modules = Vec::new();
    for (m, present) in rows.iter().zip(&lives) {
        let prs = parse_versions(m.pr_versions.as_deref());
        let end = ends.get(m.technical_name.as_str());
        let lives_on = end.filter(|(n, ..)| at_to.contains_key(n.as_str()));
        // A PR never outranks the module being there (`is_new` is per base
        // branch), nor OpenUpgrade's word that it is, under another name.
        let status = match (m.scope_from, present.contains(&to)) {
            (true, true) => "ported",
            (true, false) if lives_on.is_some() => "renamed",
            (true, false) if prs.contains(&to) => "pr",
            (true, false) => "not_started",
            // At `to` in this workspace but at `from` only in another: it
            // moved in, neither new nor this workspace's to migrate.
            (false, _) if present.contains(&from) => continue,
            (false, _) => "new",
        };
        let first = status == "new"
            && !present.iter().any(|v| *v < to)
            && !replaces.contains_key(m.technical_name.as_str());
        tally.add(status, first);
        repo_tally
            .entry((m.org_name.clone(), m.repository.clone()))
            .or_default()
            .add(status, first);
        if status == "ported" {
            continue;
        }
        let upstream = match end {
            Some((name, merged, at)) => Some(Upstream {
                kind: if *merged { "merged" } else { "renamed" },
                org_name: at_to.get(name.as_str()).map(|o| o.to_string()),
                name: name.clone(),
                version: version_str(*at),
            }),
            None if status == "new" => {
                replaces.get(m.technical_name.as_str()).map(|old| Upstream {
                    kind: "replaces",
                    name: old.join(", "),
                    version: version_str(to),
                    org_name: None,
                })
            }
            None => None,
        };
        let renamed_at = lives_on.map(|(.., at)| *at);
        let in_pr = |v: i32| prs.contains(&v) && !present.contains(&v);
        let mut life = runs(&versions, |v| present.contains(&v));
        let pr_life = runs(&versions, in_pr);
        if !pr_life.is_empty() {
            life = format!("{life} · PR {pr_life}");
        }
        if let Some(u) = upstream.as_ref().filter(|u| u.kind != "replaces") {
            let verb = if u.kind == "merged" {
                "merged into"
            } else {
                "renamed to"
            };
            life = format!("{life} · {verb} {} in {}", u.name, u.version);
        }
        modules.push(MigrationModule {
            cells: versions
                .iter()
                .map(|v| match (present.contains(v), in_pr(*v)) {
                    (true, _) => 2,
                    _ if renamed_at.is_some_and(|at| at <= *v && *v <= to) => 3,
                    (_, true) => 1,
                    _ => 0,
                })
                .collect(),
            life,
            technical_name: m.technical_name.clone(),
            org_name: m.org_name.clone(),
            repository: m.repository.clone(),
            status,
            dependents: m.dependents,
            first,
            pr: None,
            upstream,
        });
    }

    let names: Vec<String> = modules
        .iter()
        .filter(|m| m.status == "pr")
        .map(|m| m.technical_name.clone())
        .collect();
    let mut prs: HashMap<String, MigrationPendingModuleInfo> =
        get_pending_modules(all, &names, &(to as u8))
            .into_iter()
            .map(|pr| (pr.technical_name.clone(), pr))
            .collect();
    for m in modules.iter_mut().filter(|m| m.status == "pr") {
        m.pr = prs.remove(&m.technical_name);
    }

    let mut counts: BTreeMap<(String, String), HashMap<i32, i64>> = BTreeMap::new();
    for c in models::module::repository_version_counts(conn) {
        counts
            .entry((c.org_name, c.repository))
            .or_default()
            .insert(c.version_odoo, c.modules);
    }
    let mut repos: Vec<MigrationRepo> = repo_tally
        .into_iter()
        .map(|(key, t)| {
            let by_version = counts.remove(&key).unwrap_or_default();
            let max = by_version.values().copied().max().unwrap_or(0);
            let cells = versions
                .iter()
                .map(|v| {
                    let modules = by_version.get(v).copied().unwrap_or(0);
                    RepoCell {
                        modules,
                        level: if max > 0 {
                            (4 * modules + max - 1) / max
                        } else {
                            0
                        },
                    }
                })
                .collect();
            MigrationRepo {
                org_name: key.0,
                repository: key.1,
                cells,
                tally: t.finish(),
            }
        })
        .collect();
    repos.sort_by_cached_key(|r| {
        (
            Reverse(r.tally.pr + r.tally.not_started),
            Reverse(r.tally.from_modules),
            (r.org_name.clone(), r.repository.clone()),
        )
    });

    let idx = |v: i32| versions.iter().position(|x| *x == v).unwrap_or_default();
    MigrationData {
        from_idx: idx(from),
        to_idx: idx(to),
        versions: versions.into_iter().map(version_str).collect(),
        from: version_str(from),
        to: version_str(to),
        tally: tally.finish(),
        modules,
        repos,
    }
}

#[derive(Debug, Deserialize)]
pub struct RouteMigrationRequest {
    from: Option<String>,
    to: Option<String>,
    /// `pending` (default: `pr` + `not_started`), `pr`, `not_started`,
    /// `renamed`, `new`, `first` (of `new`, at no earlier version).
    status: Option<String>,
    /// `org/repository`.
    repo: Option<String>,
}

#[get("/migration")]
pub async fn route(
    pool: AreaPool,
    all: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    info: web::Query<RouteMigrationRequest>,
) -> Result<impl Responder> {
    let info = info.into_inner();
    let data = web::block(move || {
        let mut conn = pool.get().unwrap();
        let mut all = all.get().unwrap();
        let versions = models::module::get_odoo_versions(&mut conn);
        // Unknown or unparsable versions fall back to the newest pair.
        let (from, to) =
            pick_compare_versions(&versions, info.from.as_deref(), info.to.as_deref())?;
        Some(compute_migration_data(
            &mut conn,
            &mut all,
            pool.key(),
            &versions,
            from,
            to,
        ))
    })
    .await?;

    // Whitelisted: it goes back into the page's links unencoded.
    let status = info
        .status
        .as_deref()
        .filter(|s| ["renamed", "pr", "not_started", "new", "first"].contains(s))
        .unwrap_or("pending");
    let repo = info.repo.filter(|r| !r.is_empty());
    let rows: Vec<&MigrationModule> = data
        .iter()
        .flat_map(|d| &d.modules)
        .filter(|m| match status {
            "pending" => matches!(m.status, "pr" | "not_started"),
            "first" => m.first,
            s => m.status == s,
        })
        .filter(|m| {
            repo.as_deref()
                .is_none_or(|r| r == format!("{}/{}", m.org_name, m.repository))
        })
        .collect();
    let top_dependents = rows.iter().map(|m| m.dependents).max().unwrap_or(0);
    // The status tabs count within the repository filter, when there is one.
    let scope = data.as_ref().map(|d| {
        d.repos
            .iter()
            .find(|r| repo.as_deref() == Some(&format!("{}/{}", r.org_name, r.repository)))
            .map_or(&d.tally, |r| &r.tally)
    });

    tmpl_env.render(
        "pages/migration.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "migration",
                d => data,
                rows => rows,
                scope => scope,
                top_dependents => top_dependents,
                status => status,
                repo => repo,
            )
        ),
    )
}

#[cfg(test)]
mod tests {
    use sqlitedb::fixture::demo;
    use sqlitedb::models::module;

    // Fixture: 17.0 = web_widget_demo; 18.0 = sale, web_widget_demo, sale_demo.
    #[test]
    fn transition_agrees_with_the_overview_coverage() {
        let (_db, pool) = demo("migration-transition");
        let mut conn = pool.get().unwrap();
        let versions = module::get_odoo_versions(&mut conn);
        let d = super::compute_migration_data(
            &mut conn,
            &mut pool.get().unwrap(),
            "",
            &versions,
            170,
            180,
        );
        let c = &module::version_coverage(&mut conn)[0];
        let t = &d.tally;
        assert_eq!(
            (t.from_modules, t.ported, t.renamed, t.pr, t.new),
            (
                c.prev_modules,
                c.ported,
                c.renamed,
                c.in_progress,
                c.new_modules
            )
        );
        assert_eq!(d.versions, ["17.0", "18.0"]);
        let new: Vec<_> = d
            .modules
            .iter()
            .map(|m| (m.technical_name.as_str(), m.status, m.cells.clone()))
            .collect();
        assert_eq!(
            new,
            [
                ("sale", "new", vec![0, 2]),
                ("sale_demo", "new", vec![0, 2])
            ]
        );

        // Backwards, 17.0 lacks two 18.0 modules: sale has one dependent.
        let d = super::compute_migration_data(
            &mut conn,
            &mut pool.get().unwrap(),
            "",
            &versions,
            180,
            170,
        );
        let missing: Vec<_> = d
            .modules
            .iter()
            .map(|m| {
                (
                    m.technical_name.as_str(),
                    m.status,
                    m.dependents,
                    m.life.as_str(),
                )
            })
            .collect();
        assert_eq!(
            missing,
            [
                ("sale", "not_started", 1, "18.0"),
                ("sale_demo", "not_started", 0, "18.0")
            ]
        );
        // Ties on pending work go by name: OCA before odoo, fully migrated last.
        let repos: Vec<_> = d.repos.iter().map(|r| r.repository.as_str()).collect();
        assert_eq!(repos, ["sale-workflow", "odoo", "web"]);
        assert_eq!(d.tally.ported_pct, 33);
    }

    // A copy of the fixture's 17.0 web_widget_demo, renamed, at `version_odoo`
    // in repository `repo`.
    fn add_module(write: &mut diesel::SqliteConnection, name: &str, version_odoo: i32, repo: &str) {
        use diesel::RunQueryDsl;
        diesel::sql_query(format!(
            "INSERT INTO module (technical_name, version_odoo, name, version_module, description, \
             website, license, category, auto_install, application, installable, gh_repository_id, \
             create_date, update_date, folder_size, last_commit_hash, last_commit_author, \
             last_commit_name, last_commit_date, last_commit_partof, installation, usage, icon, \
             source_path, analysis_version) \
             SELECT '{name}', {version_odoo}, name, version_module, description, website, license, \
             category, auto_install, application, installable, \
             (SELECT id FROM gh_repository WHERE name = '{repo}'), create_date, \
             update_date, folder_size, last_commit_hash, last_commit_author, last_commit_name, \
             last_commit_date, last_commit_partof, installation, usage, icon, '{name}', \
             analysis_version FROM module WHERE technical_name = 'web_widget_demo' \
             AND version_odoo = 170"
        ))
        .execute(write)
        .unwrap();
    }

    // Workspace "oca" (OCA/web, OCA/sale-workflow) lists its own modules,
    // but what odoo/odoo (workspace "core") holds still counts: a merge
    // target, a migration PR, a module that moved there or came from there.
    #[test]
    fn a_workspace_lists_its_modules_judged_by_every_workspace() {
        use diesel::RunQueryDsl;
        let (db, pool) = demo("migration-workspace");
        let mut write = sqlitedb::new_write_pool(&db.path).get().unwrap();
        add_module(&mut write, "sale_old", 170, "web");
        add_module(&mut write, "crm_old", 170, "web");
        add_module(&mut write, "moved", 170, "web");
        add_module(&mut write, "moved", 180, "odoo");
        add_module(&mut write, "moved_in", 170, "odoo");
        add_module(&mut write, "moved_in", 180, "web");
        let renames = [("sale_old".to_string(), "sale".to_string(), true)];
        sqlitedb::models::openupgrade_rename::replace_for_version(&mut write, 180, &renames)
            .unwrap();
        for sql in [
            "INSERT INTO gh_repository_workspace (gh_repository_id, workspace) \
             SELECT id, IIF(name = 'odoo', 'core', 'oca') FROM gh_repository",
            "INSERT INTO pull_request (name, version_odoo, prid, gh_repository_id, is_draft, \
             analysis_version) SELECT '[18.0][MIG] crm_old', 180, 7, id, 0, 0 \
             FROM gh_repository WHERE name = 'odoo'",
            "INSERT INTO pull_request_module (pull_request_id, module_technical_name, is_new) \
             SELECT max(id), 'crm_old', 1 FROM pull_request",
        ] {
            diesel::sql_query(sql).execute(&mut write).unwrap();
        }

        let ws = sqlitedb::new_workspace_read_pool(&db.path, 2, "oca");
        let mut conn = ws.get().unwrap();
        let versions = module::get_odoo_versions(&mut conn);
        let d = super::compute_migration_data(
            &mut conn,
            &mut pool.get().unwrap(),
            "oca",
            &versions,
            170,
            180,
        );
        let t = &d.tally;
        assert_eq!(
            (
                t.from_modules,
                t.ported,
                t.renamed,
                t.pr,
                t.not_started,
                t.new
            ),
            (4, 2, 1, 1, 0, 1)
        );
        let c = &module::version_coverage(&mut conn)[0];
        assert_eq!(
            (t.from_modules, t.ported, t.renamed, t.pr, t.new),
            (
                c.prev_modules,
                c.ported,
                c.renamed,
                c.in_progress,
                c.new_modules
            )
        );
        let listed: Vec<_> = d
            .modules
            .iter()
            .map(|m| (m.technical_name.as_str(), m.status))
            .collect();
        // No dependents anywhere: by name.
        assert_eq!(
            listed,
            [
                ("crm_old", "pr"),
                ("sale_demo", "new"),
                ("sale_old", "renamed")
            ]
        );
        let crm = d
            .modules
            .iter()
            .find(|m| m.technical_name == "crm_old")
            .unwrap();
        assert_eq!(crm.pr.as_ref().map(|pr| pr.prid), Some(7));
        let sale_old = d
            .modules
            .iter()
            .find(|m| m.technical_name == "sale_old")
            .unwrap();
        assert_eq!(
            sale_old.upstream.as_ref().unwrap().org_name.as_deref(),
            Some("odoo")
        );
        let repos: Vec<_> = d.repos.iter().map(|r| r.repository.as_str()).collect();
        assert_eq!(repos, ["web", "sale-workflow"]);
    }

    // A 17.0 module OpenUpgrade records merged into 18.0's `sale`: migrated
    // under another name, on both pages, and `sale` says what it replaces.
    #[test]
    fn openupgrade_merges_count_as_migrated() {
        let (db, pool) = demo("migration-openupgrade");
        let mut write = sqlitedb::new_write_pool(&db.path).get().unwrap();
        add_module(&mut write, "sale_old", 170, "web");
        // The second step is past `to`: not followed.
        let renames = [("sale_old".to_string(), "sale".to_string(), true)];
        sqlitedb::models::openupgrade_rename::replace_for_version(&mut write, 180, &renames)
            .unwrap();
        let later = [("sale".to_string(), "sale_new".to_string(), false)];
        sqlitedb::models::openupgrade_rename::replace_for_version(&mut write, 190, &later).unwrap();

        let mut conn = pool.get().unwrap();
        let versions = module::get_odoo_versions(&mut conn);
        // Its own cache key: the fixture's other test asks for the same pair.
        let d = super::compute_migration_data(
            &mut conn,
            &mut pool.get().unwrap(),
            "openupgrade",
            &versions,
            170,
            180,
        );
        let c = &module::version_coverage(&mut conn)[0];
        let t = &d.tally;
        assert_eq!(
            (t.from_modules, t.ported, t.renamed, t.pr, t.not_started),
            (c.prev_modules, c.ported, c.renamed, c.in_progress, 0)
        );
        assert_eq!((t.renamed, t.ported_pct), (1, 100));

        let old = d
            .modules
            .iter()
            .find(|m| m.technical_name == "sale_old")
            .unwrap();
        let u = old.upstream.as_ref().unwrap();
        assert_eq!(old.status, "renamed");
        assert_eq!(
            (u.kind, u.name.as_str(), u.org_name.as_deref()),
            ("merged", "sale", Some("odoo"))
        );
        assert_eq!(old.cells, [2, 3]);
        assert_eq!(old.life, "17.0 · merged into sale in 18.0");
        let sale = d
            .modules
            .iter()
            .find(|m| m.technical_name == "sale")
            .unwrap();
        let u = sale.upstream.as_ref().unwrap();
        assert_eq!(
            (sale.status, u.kind, u.name.as_str()),
            ("new", "replaces", "sale_old")
        );
    }

    // New in 18.0 but already at 16.0: not in 17.0, yet not new in 18.0.
    #[test]
    fn first_skips_modules_of_earlier_versions() {
        let (db, pool) = demo("migration-first");
        let mut write = sqlitedb::new_write_pool(&db.path).get().unwrap();
        add_module(&mut write, "back", 160, "web");
        add_module(&mut write, "back", 180, "web");
        let mut conn = pool.get().unwrap();
        let versions = module::get_odoo_versions(&mut conn);
        let d = super::compute_migration_data(
            &mut conn,
            &mut pool.get().unwrap(),
            "first",
            &versions,
            170,
            180,
        );
        // sale has a dependent, the rest go by name.
        let new: Vec<_> = d
            .modules
            .iter()
            .map(|m| (m.technical_name.as_str(), m.status, m.first))
            .collect();
        assert_eq!(
            new,
            [
                ("sale", "new", true),
                ("back", "new", false),
                ("sale_demo", "new", true)
            ]
        );
        assert_eq!((d.tally.new, d.tally.first), (3, 2));
    }

    #[test]
    fn runs_collapse_consecutive_versions() {
        let versions = [80, 90, 100, 110, 120];
        assert_eq!(super::runs(&versions, |v| v != 110), "8.0–10.0, 12.0");
        assert_eq!(super::runs(&versions, |_| false), "");
    }
}
