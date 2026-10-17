// Copyright Alexandre D. Díaz
//! Overview (`/`): where each Odoo version stands (modules, migration from the
//! previous version, open migration PRs, security findings), what's missing
//! in the newest one, recent activity and the sources behind it. Per-version
//! PR/security figures come from the `/modules` page data (same cache, same
//! `is_new` semantics). Collector status reads `system_event`, which no
//! workspace filters: the template shows it in dev mode only, uncached.
use std::collections::{BTreeMap, HashSet};

use actix_web::{get, web, HttpRequest, Responder, Result};
use cached::{proc_macro::cached, stores::TimedSizedCache};
use chrono::Datelike;
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::Serialize;

use crate::config::SERVER_CONFIG;
use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::routes::modules::{compute_modules_page_data, ActivePullRequestInfo};
use crate::utils::get_minijinja_context;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models;

use crate::workspace::AreaPool;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

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

#[derive(Debug, Serialize, Clone, Default)]
pub struct VersionRow {
    pub version: String,
    pub modules: i64,
    pub modules_pct: i64,
    pub prev_version: Option<String>,
    pub prev_modules: i64,
    pub ported: i64,
    pub ported_pct: i64,
    pub in_progress: i64,
    pub in_progress_pct: i64,
    pub not_started: i64,
    pub new_modules: i64,
    /// Distinct open migration PRs, and how many of them are rotten / fail CI.
    pub prs: usize,
    pub prs_rotten: usize,
    pub prs_failing: usize,
    /// Modules with more than one migration PR racing for them.
    pub duplicated: usize,
    pub avg_days_open: Option<f64>,
    pub security_errors: usize,
    pub security_warnings: usize,
    /// `/modules` only has a tab for versions with PR or findings data.
    pub has_tab: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct MonthBar {
    pub label: String,
    pub commits: i64,
    pub pct: i64,
    pub partial: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct FindingModule {
    pub org_name: String,
    pub technical_name: String,
    pub version: String,
    pub count: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct Source {
    pub name: String,
    pub repositories: i64,
    pub modules: i64,
    pub versions: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct NewModule {
    pub technical_name: String,
    pub org_name: String,
    pub version: String,
    pub create_date: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct CollectorStatus {
    pub date: String,
    pub message: String,
    /// Sources whose latest job failed.
    pub failing: Vec<String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct DashboardData {
    pub modules_total: i64,
    pub repositories_total: i64,
    pub versions: Vec<VersionRow>,
    pub prs_total: usize,
    pub prs_rotten: usize,
    pub prs_failing: usize,
    pub security_errors: usize,
    pub security_warnings: usize,
    pub osv_advisories: usize,
    pub osv_modules: usize,
    pub missing: Vec<models::module::MissingModuleInfo>,
    pub security_top: Vec<FindingModule>,
    /// The versions `security_top` covers, e.g. "17.0 – 19.0".
    pub security_top_versions: String,
    pub activity: Vec<MonthBar>,
    pub activity_month: Option<String>,
    pub top_committers: Vec<models::module_committer_period::PeriodRankEntry>,
    pub newest: Vec<NewModule>,
    pub sources: Vec<Source>,
}

/// The newest finished/failed collector job, and the sources (`'org'
/// [ver]`) whose latest job failed, so a failure followed by other sources
/// succeeding still shows.
fn collector_status(conn: &mut SqliteConnection) -> Option<CollectorStatus> {
    // ponytail: keyed on the message text `system_event.rs` formats both
    // events with; a source that hasn't run in the last 500 events drops out.
    fn source_key(message: &str) -> Option<&str> {
        let start = message.find('\'')?;
        let end = start + message[start..].find(']')? + 1;
        Some(&message[start..end])
    }
    let events = models::system_event::get_feed(conn, &["collector".to_string()], &[], 500);
    let ended = events
        .iter()
        .filter(|e| e.severity == "success" || e.severity == "error");
    let last = ended.clone().next()?;
    let mut seen = HashSet::new();
    let failing = ended
        .filter_map(|e| Some((source_key(&e.message)?, e)))
        .filter(|(key, _)| seen.insert(*key))
        .filter(|(_, e)| e.severity == "error")
        .map(|(key, _)| key.to_string())
        .collect();
    Some(CollectorStatus {
        date: last.date.clone(),
        message: last.message.clone(),
        failing,
    })
}

// Same caching as `/modules`: no query params, one entry per workspace.
#[cached(
    ty = "TimedSizedCache<String, DashboardData>",
    key = "String",
    create = r#"
        {
            let ttl_secs = *SERVER_CONFIG.get_cache_ttl();
            TimedSizedCache::with_size_and_lifespan_and_refresh(16, std::time::Duration::from_secs(ttl_secs), true)
        }
    "#,
    convert = r#"{ workspace.to_string() }"#
)]
fn compute_dashboard_data(conn: &mut SqliteConnection, workspace: &str) -> DashboardData {
    let (modules_total, groups) = compute_modules_page_data(conn, workspace);
    let groups: BTreeMap<String, _> = groups
        .into_iter()
        .map(|g| (g.odoo_version.clone(), g))
        .collect();

    let coverage = models::module::version_coverage(conn);
    let max_modules = coverage.iter().map(|c| c.modules).max().unwrap_or(0);
    let versions: Vec<VersionRow> = coverage
        .iter()
        .map(|c| {
            let version = version_str(c.version_odoo);
            let mut row = VersionRow {
                modules: c.modules,
                modules_pct: pct(c.modules, max_modules),
                prev_version: c.prev_version.map(version_str),
                prev_modules: c.prev_modules,
                ported: c.ported,
                ported_pct: pct(c.ported, c.prev_modules),
                in_progress: c.in_progress,
                in_progress_pct: pct(c.in_progress, c.prev_modules),
                not_started: c.prev_modules - c.ported - c.in_progress,
                new_modules: c.modules - c.ported,
                ..Default::default()
            };
            if let Some(g) = groups.get(&version) {
                // One PR row per module it ports: count PRs, not rows.
                let distinct = |keep: fn(&ActivePullRequestInfo) -> bool| {
                    g.pull_requests
                        .iter()
                        .filter(|pr| keep(pr))
                        .map(|pr| (&pr.organization, &pr.repository, pr.prid))
                        .collect::<HashSet<_>>()
                        .len()
                };
                row.prs = distinct(|_| true);
                row.prs_rotten = distinct(|pr| pr.freshness.as_deref() == Some("rotten"));
                row.prs_failing = distinct(|pr| pr.ci_status.as_deref() == Some("failure"));
                row.duplicated = g
                    .pull_requests
                    .iter()
                    .filter(|pr| pr.is_duplicate)
                    .map(|pr| &pr.module_technical_name)
                    .collect::<HashSet<_>>()
                    .len();
                row.avg_days_open = g.avg_days_open;
                row.security_errors = g.security_errors.len();
                row.security_warnings = g.security_warnings.len();
                row.has_tab = true;
            }
            row.version = version;
            row
        })
        .collect();

    let missing = match coverage.first() {
        Some(models::module::VersionCoverage {
            version_odoo,
            prev_version: Some(prev),
            ..
        }) => models::module::missing_in_version(conn, *version_odoo, *prev, 10),
        _ => Vec::new(),
    };

    // Odoo supports its last three major versions: older findings are history.
    let recent: Vec<&String> = versions.iter().take(3).map(|v| &v.version).collect();
    let mut security_top: BTreeMap<(String, String, String), usize> = BTreeMap::new();
    for g in groups
        .values()
        .filter(|g| recent.contains(&&g.odoo_version))
    {
        for w in &g.security_errors {
            *security_top
                .entry((
                    w.organization.clone(),
                    w.technical_name.clone(),
                    g.odoo_version.clone(),
                ))
                .or_insert(0) += 1;
        }
    }
    let mut security_top: Vec<FindingModule> = security_top
        .into_iter()
        .map(
            |((org_name, technical_name, version), count)| FindingModule {
                org_name,
                technical_name,
                version,
                count,
            },
        )
        .collect();
    security_top.sort_by_key(|m| std::cmp::Reverse(m.count));
    security_top.truncate(8);
    let security_top_versions = match recent.as_slice() {
        [] => String::new(),
        [only] => only.to_string(),
        [newest, .., oldest] => format!("{oldest} – {newest}"),
    };

    let osv = models::dependency_osv::get_osv_info(conn);
    let osv_advisories = osv.iter().map(|o| &o.osv_id).collect::<HashSet<_>>().len();
    let osv_modules = osv
        .iter()
        .map(|o| (&o.org_name, &o.module_technical_name))
        .collect::<HashSet<_>>()
        .len();

    let months = models::module_committer_period::monthly_commits(conn, 12);
    let max_commits = months.iter().map(|m| m.commits).max().unwrap_or(0);
    let now = chrono::Utc::now();
    let activity = months
        .iter()
        .map(|m| MonthBar {
            label: format!("{} {}", MONTHS[(m.month - 1) as usize], m.year),
            commits: m.commits,
            pct: pct(m.commits, max_commits),
            partial: m.year == now.year() && m.month == now.month() as i32,
        })
        .collect::<Vec<_>>();
    let (top_committers, activity_month) = match months.last() {
        Some(m) => (
            models::module_committer_period::rank_between(
                conn,
                (m.year, m.month),
                (m.year, m.month),
                5,
            ),
            activity.last().map(|a| a.label.clone()),
        ),
        None => (Vec::new(), None),
    };

    let newest = models::module::get_latest_modules_created(conn)
        .into_iter()
        .take(6)
        .map(|m| NewModule {
            technical_name: m.technical_name,
            org_name: m.org_name,
            version: version_str(m.version_odoo),
            create_date: m.create_date,
        })
        .collect();

    let summary = models::gh_organization::summary(conn);
    let repositories_total = summary.iter().map(|s| s.repositories).sum();
    let sources = summary
        .into_iter()
        .map(|s| Source {
            name: s.name,
            repositories: s.repositories,
            modules: s.modules,
            versions: if s.min_version == s.max_version {
                version_str(s.min_version)
            } else {
                format!(
                    "{} – {}",
                    version_str(s.min_version),
                    version_str(s.max_version)
                )
            },
        })
        .collect();

    DashboardData {
        modules_total,
        repositories_total,
        prs_total: versions.iter().map(|v| v.prs).sum(),
        prs_rotten: versions.iter().map(|v| v.prs_rotten).sum(),
        prs_failing: versions.iter().map(|v| v.prs_failing).sum(),
        security_errors: versions.iter().map(|v| v.security_errors).sum(),
        security_warnings: versions.iter().map(|v| v.security_warnings).sum(),
        versions,
        osv_advisories,
        osv_modules,
        missing,
        security_top,
        security_top_versions,
        activity,
        activity_month,
        top_committers,
        newest,
        sources,
    }
}

#[get("/")]
pub async fn route(
    pool: AreaPool,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    // Collector status stays out of the cache: it's cheap and must be fresh.
    let (data, collector) = web::block(move || {
        let mut conn = pool.get().unwrap();
        (
            compute_dashboard_data(&mut conn, pool.key()),
            collector_status(&mut conn),
        )
    })
    .await?;

    tmpl_env.render(
        "pages/dashboard.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "dashboard",
                d => data,
                collector => collector,
            )
        ),
    )
}

#[cfg(test)]
mod tests {
    use sqlitedb::fixture::demo;
    use sqlitedb::models::{module, system_event};

    #[test]
    fn a_source_whose_last_job_failed_shows_after_others_succeed() {
        let (db, pool) = demo("dashboard-collector");
        let mut write = sqlitedb::new_write_pool(&db.path).get().unwrap();
        system_event::register_finished_task_collector(&mut write, "9", "1", "OCA", "17.0")
            .unwrap();
        system_event::register_failed_task_collector(&mut write, "OCA", "17.0", "boom").unwrap();
        system_event::register_finished_task_collector(&mut write, "9", "3", "OCA", "18.0")
            .unwrap();
        let status = super::collector_status(&mut pool.get().unwrap()).unwrap();
        assert_eq!(status.failing, ["'OCA' [17.0]"]);
        assert!(status.message.ends_with("'OCA' [18.0]"));

        system_event::register_finished_task_collector(&mut write, "9", "1", "OCA", "17.0")
            .unwrap();
        let status = super::collector_status(&mut pool.get().unwrap()).unwrap();
        assert!(status.failing.is_empty());
    }

    // Fixture: 17.0 = web_widget_demo; 18.0 = sale, web_widget_demo, sale_demo.
    #[test]
    fn coverage_counts_modules_ported_from_the_previous_version() {
        let (_db, pool) = demo("dashboard-coverage");
        let mut conn = pool.get().unwrap();
        let rows = module::version_coverage(&mut conn);
        let summary: Vec<_> = rows
            .iter()
            .map(|r| {
                (
                    r.version_odoo,
                    r.prev_version,
                    r.modules,
                    r.prev_modules,
                    r.ported,
                )
            })
            .collect();
        assert_eq!(summary, [(180, Some(170), 3, 1, 1), (170, None, 1, 0, 0)]);
        assert!(module::missing_in_version(&mut conn, 180, 170, 10).is_empty());

        // Backwards, 17.0 lacks two 18.0 modules: sale has one dependent.
        let missing: Vec<_> = module::missing_in_version(&mut conn, 170, 180, 10)
            .into_iter()
            .map(|m| (m.technical_name, m.dependents, m.in_progress))
            .collect();
        assert_eq!(
            missing,
            [
                ("sale".to_string(), 1, false),
                ("sale_demo".to_string(), 0, false)
            ]
        );
    }
}
