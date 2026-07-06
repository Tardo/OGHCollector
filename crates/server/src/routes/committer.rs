// Copyright Alexandre D. Díaz
use std::collections::{HashMap, HashSet};

use actix_web::{get, web, HttpRequest, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use chrono::Month;
use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

fn month_year_label(year: i32, month: i32) -> String {
    let month_name = u8::try_from(month)
        .ok()
        .and_then(|m| Month::try_from(m).ok())
        .map(|m| m.name())
        .unwrap_or("?");
    format!("{month_name} {year}")
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterModuleRow {
    pub technical_name: String,
    pub name: String,
    pub organization: String,
    pub repository: String,
    pub commits: i32,
    pub insertions: i32,
    pub deletions: i32,
}

/// Rough "how much text is that" fun fact from total lines added. Both
/// constants are ballpark estimates (source lines aren't prose), good enough
/// for a trivia line, not a serious measurement.
/// ponytail: naive heuristic, revisit if this should ever be exact.
fn quijote_fun_fact(total_insertions: i64) -> Option<String> {
    if total_insertions <= 0 {
        return None;
    }
    const AVG_CHARS_PER_LINE: f64 = 45.0;
    const QUIJOTE_CHARS: f64 = 2_000_000.0;
    const PAGE_CHARS: f64 = 2_000.0;

    let total_chars = total_insertions as f64 * AVG_CHARS_PER_LINE;
    let quijotes = total_chars / QUIJOTE_CHARS;
    let pages = total_chars / PAGE_CHARS;

    Some(if pages < 1.0 {
        format!("{total_chars:.0} characters written so far - not quite a full book page yet.")
    } else if quijotes < 0.1 {
        format!("About {pages:.0} pages written - roughly a short story's worth of text.")
    } else if quijotes < 1.0 {
        format!(
            "Enough text for {:.0}% of a Don Quixote ({pages:.0} pages so far).",
            quijotes * 100.0
        )
    } else if quijotes < 10.0 {
        format!("Enough text to write {quijotes:.1} copies of Don Quixote.")
    } else {
        format!(
            "Enough text for {quijotes:.0} copies of Don Quixote - practically a library shelf."
        )
    })
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterVersionGroup {
    pub odoo_version: String,
    pub version_key: i32,
    pub total_commits: i64,
    pub modules: Vec<CommitterModuleRow>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterRepoStat {
    pub organization: String,
    pub repository: String,
    pub commits: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterFunFacts {
    pub first_seen: String,
    pub first_module_name: String,
    pub first_module_technical_name: String,
    pub first_module_organization: String,
    pub last_seen: String,
    pub last_module_name: String,
    pub last_module_technical_name: String,
    pub last_module_organization: String,
    pub busiest_period: String,
    pub busiest_period_commits: i32,
    pub active_span: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct CommitterStats {
    pub total_commits: i64,
    pub total_modules: usize,
    pub total_repositories: usize,
    pub total_organizations: usize,
    pub total_insertions: i64,
    pub total_deletions: i64,
    pub versions: Vec<CommitterVersionGroup>,
    pub best_version: Option<String>,
    pub best_version_commits: i64,
    pub top_repos: Vec<CommitterRepoStat>,
    pub global_rank: Option<i64>,
    pub total_committers: Option<i64>,
    pub fun_facts: Option<CommitterFunFacts>,
    pub quijote_fun_fact: Option<String>,
}

fn build_fun_facts(conn: &mut SqliteConnection, name: &str) -> Option<CommitterFunFacts> {
    let periods = models::module_committer_period::get_activity_by_committer_name(conn, name);
    let first = periods.first()?;
    let last = periods.last()?;

    let busiest = periods.iter().max_by_key(|p| p.commits)?;

    let months_active = (last.year - first.year) * 12 + (last.month - first.month);
    let (years, months) = (months_active / 12, months_active % 12);
    let active_span = match (years, months) {
        (0, 0) => "less than a month".to_string(),
        (0, m) => format!("{m} month{}", if m != 1 { "s" } else { "" }),
        (y, 0) => format!("{y} year{}", if y != 1 { "s" } else { "" }),
        (y, m) => format!(
            "{y} year{}, {m} month{}",
            if y != 1 { "s" } else { "" },
            if m != 1 { "s" } else { "" }
        ),
    };

    Some(CommitterFunFacts {
        first_seen: month_year_label(first.year, first.month),
        first_module_name: first.name.clone(),
        first_module_technical_name: first.technical_name.clone(),
        first_module_organization: first.organization.clone(),
        last_seen: month_year_label(last.year, last.month),
        last_module_name: last.name.clone(),
        last_module_technical_name: last.technical_name.clone(),
        last_module_organization: last.organization.clone(),
        busiest_period: month_year_label(busiest.year, busiest.month),
        busiest_period_commits: busiest.commits,
        active_span,
    })
}

fn build_committer_stats(conn: &mut SqliteConnection, name: &str) -> CommitterStats {
    let rows = models::module_committer::get_activity_by_committer_name(conn, name);

    let mut versions: HashMap<i32, CommitterVersionGroup> = HashMap::new();
    let mut repo_totals: HashMap<(String, String), i64> = HashMap::new();
    let mut modules_seen: HashSet<String> = HashSet::new();
    let mut repos_seen: HashSet<(String, String)> = HashSet::new();
    let mut orgs_seen: HashSet<String> = HashSet::new();
    let mut total_commits: i64 = 0;
    let mut total_insertions: i64 = 0;
    let mut total_deletions: i64 = 0;

    for row in &rows {
        total_commits += row.commits as i64;
        total_insertions += row.insertions as i64;
        total_deletions += row.deletions as i64;
        modules_seen.insert(row.technical_name.clone());
        repos_seen.insert((row.organization.clone(), row.repository.clone()));
        orgs_seen.insert(row.organization.clone());

        let entry = versions
            .entry(row.version_odoo)
            .or_insert_with(|| CommitterVersionGroup {
                odoo_version: odoo_version_u8_to_string(&(row.version_odoo as u8)),
                version_key: row.version_odoo,
                total_commits: 0,
                modules: Vec::new(),
            });
        entry.total_commits += row.commits as i64;
        entry.modules.push(CommitterModuleRow {
            technical_name: row.technical_name.clone(),
            name: row.name.clone(),
            organization: row.organization.clone(),
            repository: row.repository.clone(),
            commits: row.commits,
            insertions: row.insertions,
            deletions: row.deletions,
        });

        *repo_totals
            .entry((row.organization.clone(), row.repository.clone()))
            .or_insert(0) += row.commits as i64;
    }

    let mut versions: Vec<CommitterVersionGroup> = versions.into_values().collect();
    versions.sort_by_key(|v| std::cmp::Reverse(v.version_key));
    for version in versions.iter_mut() {
        version
            .modules
            .sort_by_key(|m| std::cmp::Reverse(m.commits));
    }

    let best = versions.iter().max_by_key(|v| v.total_commits);
    let best_version = best.map(|v| v.odoo_version.clone());
    let best_version_commits = best.map(|v| v.total_commits).unwrap_or(0);

    let mut top_repos: Vec<CommitterRepoStat> = repo_totals
        .into_iter()
        .map(|((organization, repository), commits)| CommitterRepoStat {
            organization,
            repository,
            commits,
        })
        .collect();
    top_repos.sort_by_key(|r| std::cmp::Reverse(r.commits));
    top_repos.truncate(5);

    let rank_info = models::committer::get_global_rank_by_name(conn, name);
    let fun_facts = build_fun_facts(conn, name);

    CommitterStats {
        total_commits,
        total_modules: modules_seen.len(),
        total_repositories: repos_seen.len(),
        total_organizations: orgs_seen.len(),
        total_insertions,
        total_deletions,
        versions,
        best_version,
        best_version_commits,
        top_repos,
        global_rank: rank_info.as_ref().map(|r| r.rank),
        total_committers: rank_info.as_ref().map(|r| r.total_committers),
        fun_facts,
        quijote_fun_fact: quijote_fun_fact(total_insertions),
    }
}

#[get("/committer/{name}")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: web::Data<Pool>,
    path: web::Path<String>,
) -> Result<impl Responder> {
    let name = path.into_inner();
    let name_ctx = name.clone();
    let stats = web::block(move || {
        let mut conn = pool.get().unwrap();
        build_committer_stats(&mut conn, &name)
    })
    .await?;

    tmpl_env.render(
        "pages/committer.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "committer",
                committer_name => name_ctx,
                stats => stats,
            )
        ),
    )
}
