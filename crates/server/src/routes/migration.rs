// Copyright Alexandre D. Díaz
//! Version migration (`/migration?from=17.0&to=18.0`): where the installable
//! modules of one Odoo version stand in another (migrated, in an open
//! migration PR, not started, or new there), per repository (its module
//! count at every version) and per module (the versions it exists at). Same
//! name = same module and `is_new` PRs, as the Overview, which takes its
//! "In X, not in Y" list from here so both pages agree. Listing route:
//! `AreaPool`, cache keyed on `pool.key()`. Migrated modules are counted,
//! never listed: thousands of rows nobody has to act on.
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
use sqlitedb::models;

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
    /// `pr` | `not_started` | `new`.
    pub status: &'static str,
    pub dependents: i64,
    /// Per indexed version (ascending): 2 installable, 1 open migration PR, 0 neither.
    pub cells: Vec<u8>,
    /// The same, in words: "8.0–16.0, 18.0 · PR 19.0".
    pub life: String,
    pub pr: Option<MigrationPendingModuleInfo>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct Tally {
    pub from_modules: i64,
    pub ported: i64,
    pub pr: i64,
    pub not_started: i64,
    pub new: i64,
    pub ported_pct: i64,
    pub pr_pct: i64,
}

impl Tally {
    fn add(&mut self, status: &str) {
        match status {
            "new" => self.new += 1,
            "ported" => self.ported += 1,
            "pr" => self.pr += 1,
            _ => self.not_started += 1,
        }
        if status != "new" {
            self.from_modules += 1;
        }
    }

    fn finish(mut self) -> Self {
        self.ported_pct = pct(self.ported, self.from_modules);
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
    /// Not migrated (`pr`, `not_started`) and `new`, most depended-on first.
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
    _workspace: &str, // cache key only
    versions_desc: &[i32],
    from: i32,
    to: i32,
) -> MigrationData {
    let versions: Vec<i32> = versions_desc.iter().rev().copied().collect();
    let mut tally = Tally::default();
    let mut repo_tally: HashMap<(String, String), Tally> = HashMap::new();
    let mut modules = Vec::new();
    for m in models::module::version_transition(conn, from, to) {
        let present = parse_versions(Some(&m.versions));
        let prs = parse_versions(m.pr_versions.as_deref());
        // A PR never outranks the module being there (`is_new` is per base branch).
        let status = match (m.in_from, m.in_to) {
            (true, true) => "ported",
            (true, false) if prs.contains(&to) => "pr",
            (true, false) => "not_started",
            _ => "new",
        };
        tally.add(status);
        repo_tally
            .entry((m.org_name.clone(), m.repository.clone()))
            .or_default()
            .add(status);
        if status == "ported" {
            continue;
        }
        let in_pr = |v: i32| prs.contains(&v) && !present.contains(&v);
        let mut life = runs(&versions, |v| present.contains(&v));
        let pr_life = runs(&versions, in_pr);
        if !pr_life.is_empty() {
            life = format!("{life} · PR {pr_life}");
        }
        modules.push(MigrationModule {
            cells: versions
                .iter()
                .map(|v| match (present.contains(v), in_pr(*v)) {
                    (true, _) => 2,
                    (_, true) => 1,
                    _ => 0,
                })
                .collect(),
            life,
            technical_name: m.technical_name,
            org_name: m.org_name,
            repository: m.repository,
            status,
            dependents: m.dependents,
            pr: None,
        });
    }

    let names: Vec<String> = modules
        .iter()
        .filter(|m| m.status == "pr")
        .map(|m| m.technical_name.clone())
        .collect();
    let mut prs: HashMap<String, MigrationPendingModuleInfo> =
        get_pending_modules(conn, &names, &(to as u8))
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
    /// `pending` (default: `pr` + `not_started`), `pr`, `not_started`, `new`.
    status: Option<String>,
    /// `org/repository`.
    repo: Option<String>,
}

#[get("/migration")]
pub async fn route(
    pool: AreaPool,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    info: web::Query<RouteMigrationRequest>,
) -> Result<impl Responder> {
    let info = info.into_inner();
    let data = web::block(move || {
        let mut conn = pool.get().unwrap();
        let versions = models::module::get_odoo_versions(&mut conn);
        // Unknown or unparsable versions fall back to the newest pair.
        let (from, to) =
            pick_compare_versions(&versions, info.from.as_deref(), info.to.as_deref())?;
        Some(compute_migration_data(
            &mut conn,
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
        .filter(|s| ["pr", "not_started", "new"].contains(s))
        .unwrap_or("pending");
    let repo = info.repo.filter(|r| !r.is_empty());
    let rows: Vec<&MigrationModule> = data
        .iter()
        .flat_map(|d| &d.modules)
        .filter(|m| match status {
            "pending" => m.status != "new",
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
        let d = super::compute_migration_data(&mut conn, "", &versions, 170, 180);
        let c = &module::version_coverage(&mut conn)[0];
        let t = &d.tally;
        assert_eq!(
            (t.from_modules, t.ported, t.pr, t.new),
            (
                c.prev_modules,
                c.ported,
                c.in_progress,
                c.modules - c.ported
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
        let d = super::compute_migration_data(&mut conn, "", &versions, 180, 170);
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

    #[test]
    fn runs_collapse_consecutive_versions() {
        let versions = [80, 90, 100, 110, 120];
        assert_eq!(super::runs(&versions, |v| v != 110), "8.0–10.0, 12.0");
        assert_eq!(super::runs(&versions, |_| false), "");
    }
}
