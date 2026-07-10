// Copyright Alexandre D. Díaz
use std::collections::{BTreeMap, HashMap};

use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ActivePullRequestInfo {
    pub title: String,
    pub prid: i64,
    pub organization: String,
    pub repository: String,
    pub module_technical_name: String,
    pub url: String,
    pub age_days: Option<i64>,
    pub last_message_days: Option<i64>,
    pub freshness: Option<String>,
    pub ci_status: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleSecurityFindingInfo {
    pub code: String,
    pub message: String,
    pub xml_id: Option<String>,
    pub organization: String,
    pub technical_name: String,
}

// One Odoo-version tab's worth of content, so the template only has to loop
// once over versions instead of filtering three flat lists per tab.
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ModulesVersionGroup {
    pub odoo_version: String,
    pub pull_requests: Vec<ActivePullRequestInfo>,
    pub pr_fresh: usize,
    pub pr_rotting: usize,
    pub pr_rotten: usize,
    pub security_errors: Vec<ModuleSecurityFindingInfo>,
    pub security_warnings: Vec<ModuleSecurityFindingInfo>,
    pub avg_days_open: Option<f64>,
    pub closed_count: i64,
}

// Freshness is judged by how long a PR/MR has been quiet (days since its
// last message/activity), scaled to how long a PR at that Odoo version
// typically takes to merge (`avg_days_open`, from `pull_request_history`):
// "fresh" for the first quarter of a typical merge cycle, "rotting" until
// it's been quiet as long as a full cycle, "rotten" past that. A fixed
// fallback applies when a version doesn't have enough merge history yet to
// compute an average. PRs with unknown last-message date don't count toward
// any bucket.
const PR_FRESH_MAX_DAYS_DEFAULT: f64 = 7.0;
const PR_ROTTING_MAX_DAYS_DEFAULT: f64 = 30.0;

fn freshness(last_message_days: Option<i64>, avg_days_open: Option<f64>) -> Option<&'static str> {
    let days = last_message_days? as f64;
    let (fresh_max, rotting_max) = match avg_days_open {
        Some(avg) if avg > 0.0 => (avg / 4.0, avg),
        _ => (PR_FRESH_MAX_DAYS_DEFAULT, PR_ROTTING_MAX_DAYS_DEFAULT),
    };
    Some(if days <= fresh_max {
        "fresh"
    } else if days <= rotting_max {
        "rotting"
    } else {
        "rotten"
    })
}

fn get_group(
    by_version: &mut BTreeMap<i32, ModulesVersionGroup>,
    version_odoo: i32,
) -> &mut ModulesVersionGroup {
    by_version
        .entry(version_odoo)
        .or_insert_with(|| ModulesVersionGroup {
            odoo_version: odoo_version_u8_to_string(&(version_odoo as u8)),
            ..Default::default()
        })
}

#[get("/modules")]
pub async fn route(
    pool: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    let (
        modules_total,
        version_groups,
        most_changed,
        most_contributors,
        broadest_reach,
        most_relied_upon,
    ) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules_total = models::module::count_distinct(&mut conn);
        let mut by_version: BTreeMap<i32, ModulesVersionGroup> = BTreeMap::new();

        // Computed first: PR freshness below is scaled to each version's
        // average time-to-merge, and the page also displays this figure directly.
        let mut avg_days_open_by_version: HashMap<i32, f64> = HashMap::new();
        for stat in models::pull_request_history::average_days_open_by_version(&mut conn) {
            let group = get_group(&mut by_version, stat.version_odoo);
            group.avg_days_open = Some(stat.avg_days);
            group.closed_count = stat.closed_count;
            avg_days_open_by_version.insert(stat.version_odoo, stat.avg_days);
        }

        for pr in models::pull_request::get_all(&mut conn) {
            let last_message_days = models::pull_request::days_since(pr.last_message_at.as_deref());
            let pr_freshness = freshness(
                last_message_days,
                avg_days_open_by_version.get(&pr.version_odoo).copied(),
            );
            let entry = ActivePullRequestInfo {
                url: format!(
                    "https://github.com/{}/{}/pull/{}",
                    pr.org_name, pr.repository_name, pr.prid
                ),
                age_days: models::pull_request::days_since(pr.created_at.as_deref()),
                last_message_days,
                freshness: pr_freshness.map(str::to_string),
                ci_status: pr.ci_status,
                title: pr.name,
                prid: pr.prid,
                organization: pr.org_name,
                repository: pr.repository_name,
                module_technical_name: pr.module_technical_name,
            };
            let group = get_group(&mut by_version, pr.version_odoo);
            match pr_freshness {
                Some("fresh") => group.pr_fresh += 1,
                Some("rotting") => group.pr_rotting += 1,
                Some("rotten") => group.pr_rotten += 1,
                _ => {}
            }
            group.pull_requests.push(entry);
        }

        // "error" is grave, "warning" is minor (see module_security_warning
        // model doc); both are shown here, split by severity per version.
        for w in models::module_security_warning::get_all_current(&mut conn) {
            let is_error = w.severity == models::module_security_warning::SEVERITY_ERROR;
            let entry = ModuleSecurityFindingInfo {
                code: w.code,
                message: w.message,
                xml_id: w.xml_id,
                organization: w.org_name,
                technical_name: w.technical_name,
            };
            let group = get_group(&mut by_version, w.version_odoo);
            if is_error {
                group.security_errors.push(entry);
            } else {
                group.security_warnings.push(entry);
            }
        }

        // Newest Odoo version first.
        let version_groups = by_version.into_values().rev().collect::<Vec<_>>();

        let most_changed = models::module::most_changed(&mut conn);
        let most_contributors = models::module::most_contributors(&mut conn);
        let broadest_reach = models::module::broadest_reach(&mut conn);
        let most_relied_upon = models::module::most_relied_upon(&mut conn);

        (
            modules_total,
            version_groups,
            most_changed,
            most_contributors,
            broadest_reach,
            most_relied_upon,
        )
    })
    .await?;

    tmpl_env.render(
        "pages/modules.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "modules",
                modules_total => modules_total,
                version_groups => version_groups,
                most_changed => most_changed,
                most_contributors => most_contributors,
                broadest_reach => broadest_reach,
                most_relied_upon => most_relied_upon,
            )
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::freshness;

    #[test]
    fn test_freshness_scales_with_avg_days_open() {
        // avg 20 days to merge: fresh <= 5, rotting <= 20, rotten > 20.
        assert_eq!(freshness(Some(3), Some(20.0)), Some("fresh"));
        assert_eq!(freshness(Some(10), Some(20.0)), Some("rotting"));
        assert_eq!(freshness(Some(21), Some(20.0)), Some("rotten"));
    }

    #[test]
    fn test_freshness_falls_back_without_history() {
        assert_eq!(freshness(Some(5), None), Some("fresh"));
        assert_eq!(freshness(Some(15), None), Some("rotting"));
        assert_eq!(freshness(Some(31), None), Some("rotten"));
        assert_eq!(freshness(None, Some(20.0)), None);
    }
}
