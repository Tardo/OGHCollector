// Copyright Alexandre D. Díaz
//! `/committers`: the people behind the tracked modules. Top committers of the
//! last 12 months and of all time, veterans (longest first-to-last span) and
//! newcomers (first commit within those 12 months). The 12 months end at the
//! latest month with data, not today, so a lagging collector never empties
//! them. Curious, not a scoreboard: no rank numbers, bots only as a share.
use std::collections::BTreeMap;

use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::Serialize;

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::routes::committer::month_year_label;
use crate::utils::get_minijinja_context;

use sqlitedb::models::{self, committer::CommitterSummary};

use crate::workspace::AreaPool;

const TOP_LIMIT: usize = 15;
/// Shorter: it sits beside the arrivals chart.
const NEWCOMER_LIMIT: usize = 10;

/// `year * 12 + month - 1` → `(year, month)`.
fn split_month(index: i32) -> (i32, i32) {
    (index.div_euclid(12), index.rem_euclid(12) + 1)
}

fn month_label(index: i32) -> String {
    let (year, month) = split_month(index);
    month_year_label(year, month)
}

/// Positions on the veterans' time axis are percentages of its width.
#[derive(Serialize)]
struct Veteran {
    name: String,
    since: i32,
    until: i32,
    span_years: f64,
    left: f64,
    width: f64,
}

#[derive(Serialize)]
struct Tick {
    year: i32,
    left: f64,
}

#[derive(Serialize)]
struct Newcomer {
    name: String,
    first_seen: String,
    commits: i64,
}

/// People whose first tracked commit falls in `year`.
#[derive(Serialize)]
struct Arrival {
    year: i32,
    people: usize,
    pct: usize,
    partial: bool,
}

#[derive(Serialize, Default)]
struct CommittersPage {
    people: usize,
    bots: usize,
    bot_share: i64,
    /// People with commits in the 12-month window, and those new in it.
    active: usize,
    new: usize,
    window: String,
    recent: Vec<models::module_committer_period::PeriodRankEntry>,
    all_time: Vec<CommitterSummary>,
    veterans: Vec<Veteran>,
    ticks: Vec<Tick>,
    newcomers: Vec<Newcomer>,
    arrivals: Vec<Arrival>,
}

/// Veterans on one time axis: whole years, from the earliest first commit to
/// the end of the latest one's year.
fn place_veterans(veterans: &[&CommitterSummary]) -> (Vec<Veteran>, Vec<Tick>) {
    let (Some(first), Some(last)) = (
        veterans.iter().map(|s| s.first_month).min(),
        veterans.iter().map(|s| s.last_month).max(),
    ) else {
        return (vec![], vec![]);
    };
    let (start_year, end_year) = (split_month(first).0, split_month(last).0 + 1);
    let axis = f64::from((end_year - start_year) * 12);
    let pos = |month: i32| f64::from(month - start_year * 12) * 100.0 / axis;
    let placed = veterans
        .iter()
        .map(|s| Veteran {
            name: s.name.clone(),
            since: split_month(s.first_month).0,
            until: split_month(s.last_month).0,
            span_years: f64::from(s.last_month - s.first_month) / 12.0,
            left: pos(s.first_month),
            width: pos(s.last_month + 1) - pos(s.first_month),
        })
        .collect();
    let ticks = (start_year..end_year)
        .filter(|year| year % 5 == 0)
        .map(|year| Tick {
            year,
            left: pos(year * 12),
        })
        .collect();
    (placed, ticks)
}

/// Everything but the 12-month ranking, from the one `activity_summary` pass.
/// `latest` is the newest month with data; the window is the 12 up to it.
fn build_page(summaries: Vec<CommitterSummary>, latest: i32) -> CommittersPage {
    let window_start = latest - 11;
    let total: i64 = summaries.iter().map(|s| s.commits).sum();
    let (people, bots): (Vec<_>, Vec<_>) = summaries.into_iter().partition(|s| s.human);
    let bot_commits: i64 = bots.iter().map(|s| s.commits).sum();

    let mut veterans: Vec<&CommitterSummary> = people.iter().collect();
    veterans.sort_by_key(|s| (std::cmp::Reverse(s.last_month - s.first_month), -s.commits));
    veterans.truncate(TOP_LIMIT);
    let (veterans, ticks) = place_veterans(&veterans);

    let mut newcomers: Vec<&CommitterSummary> = people
        .iter()
        .filter(|s| s.first_month >= window_start)
        .collect();
    let new = newcomers.len();
    newcomers.sort_by_key(|s| -s.commits);
    let newcomers = newcomers
        .into_iter()
        .take(NEWCOMER_LIMIT)
        .map(|s| Newcomer {
            name: s.name.clone(),
            first_seen: month_label(s.first_month),
            commits: s.commits,
        })
        .collect();

    let mut by_year: BTreeMap<i32, usize> = BTreeMap::new();
    for s in &people {
        *by_year.entry(split_month(s.first_month).0).or_default() += 1;
    }
    let peak = by_year.values().copied().max().unwrap_or(1);
    let (latest_year, latest_month) = split_month(latest);
    let arrivals = match (by_year.keys().next(), by_year.keys().last()) {
        (Some(&from), Some(&to)) => (from..=to)
            .map(|year| {
                let people = by_year.get(&year).copied().unwrap_or(0);
                Arrival {
                    year,
                    people,
                    pct: people * 100 / peak,
                    partial: year == latest_year && latest_month < 12,
                }
            })
            .collect(),
        _ => vec![],
    };

    let active = people
        .iter()
        .filter(|s| s.last_month >= window_start)
        .count();
    let people_count = people.len();
    let mut all_time = people;
    all_time.sort_by_key(|s| -s.commits);
    all_time.truncate(TOP_LIMIT);

    CommittersPage {
        people: people_count,
        bots: bots.len(),
        bot_share: if total > 0 {
            (bot_commits * 100 + total / 2) / total
        } else {
            0
        },
        active,
        new,
        window: format!("{} – {}", month_label(window_start), month_label(latest)),
        all_time,
        veterans,
        ticks,
        newcomers,
        arrivals,
        ..Default::default()
    }
}

#[get("/committers")]
pub async fn route(
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
    pool: AreaPool,
) -> Result<impl Responder> {
    let page = web::block(move || {
        let mut conn = pool.get().unwrap();
        let summaries = models::committer::activity_summary(&mut conn);
        let Some(latest) = summaries.iter().map(|s| s.last_month).max() else {
            return CommittersPage::default();
        };
        let window_start = latest - 11;
        let mut page = build_page(summaries, latest);
        page.recent = models::module_committer_period::rank_between(
            &mut conn,
            split_month(window_start),
            split_month(latest),
            TOP_LIMIT as i64,
        );
        page
    })
    .await?;

    tmpl_env.render(
        "pages/committers.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "committers",
                c => page,
            )
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_splits_bots_and_ranks_veterans_and_newcomers() {
        let summary = |name: &str, human, commits, first: i32, last: i32| CommitterSummary {
            name: name.into(),
            human,
            commits,
            modules: 1,
            first_month: first,
            last_month: last,
        };
        let page = build_page(
            vec![
                summary("Old", true, 10, 2010 * 12, 2026 * 12 + 7),
                summary("Busy", true, 90, 2020 * 12, 2026 * 12 + 7),
                summary("New", true, 5, 2026 * 12 + 2, 2026 * 12 + 7),
                summary("Bot", false, 105, 2000 * 12, 2026 * 12 + 7),
            ],
            2026 * 12 + 7,
        );
        assert_eq!((page.people, page.bots, page.bot_share), (3, 1, 50));
        assert_eq!((page.active, page.new), (3, 1));
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            page.veterans
                .iter()
                .map(|v| v.name.clone())
                .collect::<Vec<_>>(),
            names(&["Old", "Busy", "New"])
        );
        assert_eq!(
            (page.veterans[0].since, page.veterans[0].until),
            (2010, 2026)
        );
        assert_eq!(
            page.newcomers
                .iter()
                .map(|n| n.name.clone())
                .collect::<Vec<_>>(),
            names(&["New"])
        );
        assert_eq!(page.newcomers[0].first_seen, "March 2026");
        assert_eq!(page.all_time[0].name, "Busy");
        // Axis 2010-2026 (17 years): Old starts at 0 and runs to Aug 2026.
        let old = &page.veterans[0];
        assert_eq!(old.left, 0.0);
        assert!((old.width - 200.0 * 100.0 / 204.0).abs() < 1e-9);
        assert_eq!(
            page.ticks.iter().map(|t| t.year).collect::<Vec<_>>(),
            [2010, 2015, 2020, 2025]
        );
        assert_eq!(
            page.arrivals
                .iter()
                .map(|a| (a.year, a.people, a.partial))
                .filter(|a| a.1 > 0)
                .collect::<Vec<_>>(),
            [(2010, 1, false), (2020, 1, false), (2026, 1, true)]
        );
        assert_eq!(split_month(2026 * 12 + 7), (2026, 8));
    }
}
