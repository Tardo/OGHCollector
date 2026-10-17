// Copyright Alexandre D. Díaz
//! `/committer/{name}`: one person's footprint in the tracked modules, told as
//! trivia rather than a performance review (no rank, streaks or lines-removed
//! scores; line counts only feed the Don Quixote comparison). Modules are
//! merged across Odoo versions, one row per organization + technical name with
//! its version span, and capped at `MODULE_LIMIT`.
use std::collections::{BTreeMap, HashMap, HashSet};

use actix_web::{get, web, HttpRequest, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use serde::Serialize;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::{get_minijinja_context, thousands};

use chrono::Month;
use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::models::{self, module_committer::SharedModules};

use crate::workspace::AreaPool;

const MODULE_LIMIT: usize = 40;
/// Slots of the categorical palette in `committer.scss`; the rest is "other".
const REPO_LIMIT: usize = 6;
const CO_COMMITTER_LIMIT: i64 = 8;

fn month_name(month: i32) -> &'static str {
    u8::try_from(month)
        .ok()
        .and_then(|m| Month::try_from(m).ok())
        .map(|m| m.name())
        .unwrap_or("?")
}

pub(crate) fn month_year_label(year: i32, month: i32) -> String {
    format!("{} {year}", month_name(month))
}

/// "Oct 2016", for big-type headlines.
fn short_month_year(year: i32, month: i32) -> String {
    format!("{:.3} {year}", month_name(month))
}

fn version_label(version: i32) -> String {
    odoo_version_u8_to_string(&(version as u8))
}

/// A big-type trivia item: `headline` is the number, `caption` explains it.
#[derive(Debug, Serialize)]
pub struct Fact {
    pub headline: String,
    pub caption: String,
}

/// Rough "how much text is that" fun fact from net lines (added - removed).
/// Both constants are ballpark estimates (source lines aren't prose), good
/// enough for a trivia line, not a serious measurement.
/// ponytail: naive heuristic, revisit if this should ever be exact.
fn quijote_fun_fact(net_lines: i64) -> Option<Fact> {
    let fact = |headline: String, caption: &str| {
        Some(Fact {
            headline,
            caption: caption.to_string(),
        })
    };
    if net_lines <= 0 {
        return (net_lines < 0)
            .then(|| {
                fact(
                    format!("−{}", thousands(-net_lines)),
                    "net lines: more removed than added. A codebase janitor, not a writer",
                )
            })
            .flatten();
    }
    const AVG_CHARS_PER_LINE: f64 = 34.0;
    const QUIJOTE_CHARS: f64 = 2_000_000.0;
    const PAGE_CHARS: f64 = 2_000.0;

    let total_chars = net_lines as f64 * AVG_CHARS_PER_LINE;
    let quijotes = total_chars / QUIJOTE_CHARS;
    let pages = total_chars / PAGE_CHARS;
    let copies = format!("{quijotes:.0}×");

    if pages < 1.0 {
        fact(
            thousands(total_chars as i64),
            "characters of code: not a full book page yet",
        )
    } else if quijotes < 0.1 {
        fact(
            format!("{pages:.0} pages"),
            "of code, about a short story's worth",
        )
    } else if quijotes < 1.0 {
        fact(
            format!("{:.0}%", quijotes * 100.0),
            "of a Don Quixote, written in code",
        )
    } else if quijotes < 10.0 {
        fact(format!("{quijotes:.1}×"), "Don Quixote, written in code")
    } else if quijotes < 30.0 {
        fact(copies, "Don Quixotes of code: practically a library shelf")
    } else if quijotes < 60.0 {
        fact(copies, "Don Quixotes of code: a small bookstore's stock")
    } else if quijotes < 100.0 {
        fact(
            copies,
            "Don Quixotes of code: a small town's public library",
        )
    } else if quijotes < 200.0 {
        fact(
            copies,
            "Don Quixotes of code: Cervantes never wrote this much fan fiction",
        )
    } else {
        fact(
            copies,
            "Don Quixotes of code: at this point you ARE the Real Academia Española",
        )
    }
}

/// One module across every Odoo version the committer touched it in.
#[derive(Debug, Serialize, Clone)]
pub struct ModuleStat {
    pub organization: String,
    pub repository: String,
    pub technical_name: String,
    pub name: String,
    pub first_version: String,
    pub last_version: String,
    pub versions: usize,
    pub commits: i64,
    /// First-to-last version as a bar over the committer's own versions (%).
    pub left: f64,
    pub width: f64,
}

#[derive(Debug, Serialize)]
pub struct VersionStat {
    pub version: String,
    pub modules: usize,
    pub commits: i64,
    /// Of the busiest version.
    pub pct: i64,
}

#[derive(Debug, Serialize)]
pub struct RepoStat {
    pub organization: String,
    pub repository: String,
    /// Clone URL (GitHub or GitLab), links to the repo.
    pub url: String,
    pub modules: usize,
    pub commits: i64,
    /// Of all the committer's commits, one decimal.
    pub share: f64,
}

/// One heatmap cell; `level` 0 (none) to 4 (the busiest month).
#[derive(Debug, Serialize, Clone, Copy, Default)]
pub struct CalendarCell {
    pub commits: i64,
    pub level: u8,
}

#[derive(Debug, Serialize)]
pub struct CalendarYear {
    pub year: i32,
    pub commits: i64,
    pub months: [CalendarCell; 12],
}

/// Dated trivia, from the monthly breakdown.
#[derive(Debug, Serialize)]
pub struct Timeline {
    pub first_seen: String,
    pub first_seen_short: String,
    pub first_module_name: String,
    pub first_module_technical_name: String,
    pub first_module_organization: String,
    pub since: i32,
    pub until: i32,
    pub active_span: String,
    pub busiest_month: String,
    pub busiest_month_commits: i64,
    /// Habit, not volume: the calendar month with commits in the most years
    /// (`favourite_month_years` of `calendar.len()`). Left out under two
    /// years of activity or when it's the busiest month's own month: then it
    /// would only repeat that fact.
    pub favourite_month: Option<&'static str>,
    pub favourite_month_years: u32,
    pub calendar: Vec<CalendarYear>,
    pub busiest_year_commits: i64,
}

#[derive(Debug, Serialize, Default)]
pub struct CommitterStats {
    pub total_commits: i64,
    /// Distinct technical names, as on `/committers` (the table rows are per
    /// organization + name, so `modules.len() + more_modules` can be more).
    pub total_modules: usize,
    pub total_repositories: usize,
    pub total_organizations: usize,
    /// Oldest first.
    pub versions: Vec<VersionStat>,
    pub top_version: Option<String>,
    pub modules: Vec<ModuleStat>,
    pub more_modules: usize,
    pub longest_followed: Option<ModuleStat>,
    pub repositories: Vec<RepoStat>,
    /// Share of commits outside `repositories`.
    pub other_share: f64,
    pub co_committers: Vec<SharedModules>,
    pub timeline: Option<Timeline>,
    pub quijote: Option<Fact>,
}

fn build_timeline(periods: &[models::module_committer_period::PeriodActivity]) -> Option<Timeline> {
    let first = periods.first()?;
    let last = periods.last()?;
    let mut by_month: BTreeMap<(i32, i32), i64> = BTreeMap::new();
    let mut years: BTreeMap<i32, i64> = BTreeMap::new();
    for period in periods {
        let commits = period.commits as i64;
        *by_month.entry((period.year, period.month)).or_default() += commits;
        *years.entry(period.year).or_default() += commits;
    }
    // Ties go to the earliest month: max_by_key keeps the last maximum, so
    // walk in reverse.
    let (&(busy_year, busy_month), &busy_commits) =
        by_month.iter().rev().max_by_key(|(_, commits)| **commits)?;
    // Favourite month: years with commits in it, ties (most of the prolific
    // people are active in every month) by the median share of its year, which
    // one burst can't move. Summed commits made it the busiest month's own for
    // 77% of real committers; this way 20%, and those are left out.
    let habit = |month: i32| {
        let mut shares: Vec<f64> = (first.year..=last.year)
            .map(|year| {
                let commits = by_month.get(&(year, month)).copied().unwrap_or(0);
                commits as f64 / years.get(&year).copied().unwrap_or(0).max(1) as f64
            })
            .collect();
        shares.sort_by(f64::total_cmp);
        let n = shares.len();
        let median = (shares[(n - 1) / 2] + shares[n / 2]) / 2.0;
        (shares.iter().filter(|&&s| s > 0.0).count() as u32, median)
    };
    let habits: Vec<(u32, f64)> = (1..=12).map(habit).collect();
    let favourite = (0..12).max_by(|&a, &b| {
        (habits[a].0.cmp(&habits[b].0))
            .then(habits[a].1.total_cmp(&habits[b].1))
            .then(b.cmp(&a))
    })?;

    let span = (last.year - first.year) * 12 + (last.month - first.month);
    let plural = |n: i32, unit: &str| format!("{n} {unit}{}", if n != 1 { "s" } else { "" });
    let active_span = match span {
        0 => "less than a month".to_string(),
        1..=11 => plural(span, "month"),
        _ => plural(span / 12, "year"),
    };

    // Quartiles of the active months, as GitHub does: a few huge months
    // don't flatten every other one into the same shade.
    let mut active: Vec<i64> = by_month.values().copied().filter(|&c| c > 0).collect();
    active.sort_unstable();
    let quartiles = [1, 2, 3].map(|q| active[active.len() * q / 4]);
    let level = |commits: i64| match commits {
        0 => 0,
        c => 1 + quartiles.iter().filter(|&&q| c >= q).count() as u8,
    };
    let calendar: Vec<CalendarYear> = (first.year..=last.year)
        .map(|year| {
            let mut months = [CalendarCell::default(); 12];
            for (month, cell) in (1..).zip(months.iter_mut()) {
                let commits = by_month.get(&(year, month)).copied().unwrap_or(0);
                *cell = CalendarCell {
                    commits,
                    level: level(commits),
                };
            }
            CalendarYear {
                year,
                commits: years.get(&year).copied().unwrap_or(0),
                months,
            }
        })
        .collect();
    Some(Timeline {
        first_seen: month_year_label(first.year, first.month),
        first_module_name: first.name.clone(),
        first_module_technical_name: first.technical_name.clone(),
        first_module_organization: first.organization.clone(),
        since: first.year,
        until: last.year,
        active_span,
        first_seen_short: short_month_year(first.year, first.month),
        busiest_month: short_month_year(busy_year, busy_month),
        busiest_month_commits: busy_commits,
        favourite_month: (span >= 24 && favourite as i32 + 1 != busy_month)
            .then(|| month_name(favourite as i32 + 1)),
        favourite_month_years: habits[favourite].0,
        busiest_year_commits: calendar.iter().map(|y| y.commits).max().unwrap_or(0),
        calendar,
    })
}

fn build_committer_stats(conn: &mut SqliteConnection, name: &str) -> CommitterStats {
    // Ordered by version DESC: the first row of a module is its newest version.
    let rows = models::module_committer::get_activity_by_committer_name(conn, name);
    if rows.is_empty() {
        return CommitterStats::default();
    }

    let mut modules: HashMap<(&str, &str), ModuleStat> = HashMap::new();
    let mut versions: BTreeMap<i32, (HashSet<&str>, i64)> = BTreeMap::new();
    let mut repos: HashMap<(&str, &str), (HashSet<&str>, i64)> = HashMap::new();
    let mut organizations: HashSet<&str> = HashSet::new();
    let (mut total_commits, mut net_lines) = (0_i64, 0_i64);

    for row in &rows {
        let commits = row.commits as i64;
        total_commits += commits;
        net_lines += row.insertions as i64 - row.deletions as i64;
        organizations.insert(&row.organization);

        let version = versions.entry(row.version_odoo).or_default();
        version.0.insert(&row.technical_name);
        version.1 += commits;

        let repo = repos
            .entry((&row.organization, &row.repository))
            .or_default();
        repo.0.insert(&row.technical_name);
        repo.1 += commits;

        let module = modules
            .entry((&row.organization, &row.technical_name))
            .or_insert_with(|| ModuleStat {
                organization: row.organization.clone(),
                repository: row.repository.clone(),
                technical_name: row.technical_name.clone(),
                name: row.name.clone(),
                first_version: String::new(),
                last_version: version_label(row.version_odoo),
                versions: 0,
                commits: 0,
                left: 0.0,
                width: 0.0,
            });
        // A module's rows come newest version first, so a version repeats
        // (same name in two repositories) only right after itself.
        let label = version_label(row.version_odoo);
        if module.first_version != label {
            module.versions += 1;
            module.first_version = label;
        }
        module.commits += commits;
    }

    let mut modules: Vec<ModuleStat> = modules.into_values().collect();
    modules.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then_with(|| a.technical_name.cmp(&b.technical_name))
    });
    let longest_followed = modules
        .iter()
        .filter(|m| m.versions > 1)
        .max_by_key(|m| (m.versions, std::cmp::Reverse(&m.technical_name)))
        .cloned();
    let total_modules = modules
        .iter()
        .map(|m| &m.technical_name)
        .collect::<HashSet<_>>()
        .len();
    let module_rows = modules.len();
    modules.truncate(MODULE_LIMIT);

    let top_version = versions
        .iter()
        .max_by_key(|(version, (_, commits))| (*commits, **version))
        .map(|(version, _)| version_label(*version));
    let busiest_version = versions.values().map(|(_, c)| *c).max().unwrap_or(0).max(1);
    let versions: Vec<VersionStat> = versions
        .into_iter()
        .map(|(version, (names, commits))| VersionStat {
            version: version_label(version),
            modules: names.len(),
            commits,
            pct: commits * 100 / busiest_version,
        })
        .collect();
    let axis: HashMap<&str, usize> = versions
        .iter()
        .enumerate()
        .map(|(i, v)| (v.version.as_str(), i))
        .collect();
    let slot = 100.0 / versions.len() as f64;
    for module in &mut modules {
        let (from, to) = (
            axis[module.first_version.as_str()],
            axis[module.last_version.as_str()],
        );
        module.left = from as f64 * slot;
        module.width = (to - from + 1) as f64 * slot;
    }

    let total_repositories = repos.len();
    let repo_urls = models::gh_repository::clone_urls(conn);
    let mut repositories: Vec<RepoStat> = repos
        .into_iter()
        .map(|((organization, repository), (names, commits))| RepoStat {
            url: oghutils::git::clone_url(
                repo_urls
                    .get(&format!("{organization}/{repository}"))
                    .map(String::as_str),
                organization,
                repository,
            ),
            organization: organization.to_string(),
            repository: repository.to_string(),
            modules: names.len(),
            commits,
            share: (commits * 1000 / total_commits.max(1)) as f64 / 10.0,
        })
        .collect();
    repositories.sort_by(|a, b| {
        b.commits
            .cmp(&a.commits)
            .then_with(|| a.repository.cmp(&b.repository))
    });
    repositories.truncate(REPO_LIMIT);
    let other_share = (100.0 - repositories.iter().map(|r| r.share).sum::<f64>()).max(0.0);

    let periods = models::module_committer_period::get_activity_by_committer_name(conn, name);

    CommitterStats {
        total_commits,
        total_modules,
        total_repositories,
        total_organizations: organizations.len(),
        versions,
        top_version,
        more_modules: module_rows - modules.len(),
        modules,
        longest_followed,
        repositories,
        other_share: (other_share * 10.0).round() / 10.0,
        co_committers: models::module_committer::get_co_committers(conn, name, CO_COMMITTER_LIMIT),
        timeline: build_timeline(&periods),
        quijote: quijote_fun_fact(net_lines),
    }
}

#[get("/committer/{name}")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: AreaPool,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_aggregates_modules_per_month_and_finds_favourites() {
        let period = |year, month, commits| models::module_committer_period::PeriodActivity {
            technical_name: "module".into(),
            name: "Module".into(),
            organization: "OCA".into(),
            year,
            month,
            commits,
        };
        let periods = [
            period(2023, 3, 2),
            period(2024, 1, 4),
            period(2024, 1, 3),
            period(2024, 3, 6),
            period(2026, 2, 1),
        ];
        let timeline = build_timeline(&periods).unwrap();
        assert_eq!(
            (
                timeline.busiest_month.as_str(),
                timeline.busiest_month_commits
            ),
            ("Jan 2024", 7)
        );
        // March has commits in two years, January (the busiest) in one.
        assert_eq!(
            (timeline.favourite_month, timeline.favourite_month_years),
            (Some("March"), 2)
        );
        assert_eq!(timeline.active_span, "2 years");
        assert_eq!(
            timeline
                .calendar
                .iter()
                .map(|y| y.commits)
                .collect::<Vec<_>>(),
            [2, 13, 0, 1]
        );
        assert_eq!(timeline.busiest_year_commits, 13);
        // Active months 1, 2, 6, 7 → quartile thresholds 2, 6, 7.
        let levels = timeline.calendar[1].months.map(|cell| cell.level);
        assert_eq!(&levels[..3], &[4, 0, 3]);
        assert_eq!(timeline.calendar[0].months[2].level, 2);
        assert_eq!(timeline.calendar[3].months[1].level, 1);
        let one_year = build_timeline(&periods[1..4]).unwrap();
        assert_eq!(one_year.favourite_month, None);

        // One burst month must not become the favourite as well.
        let burst = [
            period(2023, 5, 1),
            period(2024, 1, 100),
            period(2024, 5, 1),
            period(2025, 5, 1),
        ];
        let timeline = build_timeline(&burst).unwrap();
        assert_eq!(timeline.busiest_month, "Jan 2024");
        assert_eq!(
            (timeline.favourite_month, timeline.favourite_month_years),
            (Some("May"), 3)
        );

        // A habit in the busiest month's own month would only repeat it.
        let same = [
            period(2023, 3, 5),
            period(2024, 1, 1),
            period(2024, 3, 9),
            period(2025, 3, 4),
        ];
        assert_eq!(build_timeline(&same).unwrap().favourite_month, None);
    }

    #[test]
    fn quijote_reads_as_headline_and_caption() {
        let fact = quijote_fun_fact(5_000_000).unwrap();
        assert_eq!(fact.headline, "85×");
        assert!(fact.caption.contains("public library"));
        assert_eq!(quijote_fun_fact(-1234).unwrap().headline, "−1,234");
        assert!(quijote_fun_fact(0).is_none());
    }
}
