// Copyright Alexandre D. Díaz
use std::collections::BTreeMap;

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
}

// PRs are "fresh" for their first week, start "rotting" until a month old,
// and are "rotten" past that; PRs with unknown age don't count toward any bucket.
const PR_FRESH_MAX_DAYS: i64 = 7;
const PR_ROTTING_MAX_DAYS: i64 = 30;

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

        for pr in models::pull_request::get_all(&mut conn) {
            let entry = ActivePullRequestInfo {
                url: format!(
                    "https://github.com/{}/{}/pull/{}",
                    &pr.org_name, &pr.repository_name, pr.prid
                ),
                age_days: models::pull_request::age_days(pr.created_at.as_deref()),
                ci_status: pr.ci_status,
                title: pr.name,
                prid: pr.prid,
                organization: pr.org_name,
                repository: pr.repository_name,
                module_technical_name: pr.module_technical_name,
            };
            let group = get_group(&mut by_version, pr.version_odoo);
            match entry.age_days {
                Some(days) if days <= PR_FRESH_MAX_DAYS => group.pr_fresh += 1,
                Some(days) if days <= PR_ROTTING_MAX_DAYS => group.pr_rotting += 1,
                Some(_) => group.pr_rotten += 1,
                None => {}
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
