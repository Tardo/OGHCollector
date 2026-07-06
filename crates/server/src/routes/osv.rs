// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::utils::get_minijinja_context;

use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OSVModuleRef {
    pub org_name: String,
    pub technical_name: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OSVInfo {
    pub osv_id: String,
    pub details: String,
    pub fixed_in: String,
    pub modules: Vec<OSVModuleRef>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct OSVVersionSummary {
    pub vuln_count: usize,
    pub package_count: usize,
    pub module_count: usize,
}

#[get("/osv")]
pub async fn route(
    pool: web::Data<Pool>,
    tmpl_env: MiniJinjaRenderer,
    req: HttpRequest,
) -> Result<impl Responder> {
    let (res, summary) = web::block(move || {
        let mut conn = pool.get().unwrap();
        let osv_infos = models::dependency_osv::get_osv_info(&mut conn);
        // Group package-first (not module-first): the same vulnerability is
        // shared by every module depending on the vulnerable package, so
        // grouping by module repeats each vuln once per affected module.
        let mut res: HashMap<String, HashMap<String, HashMap<String, OSVInfo>>> = HashMap::new();
        let mut affected_modules: HashMap<String, HashSet<(String, String)>> = HashMap::new();
        for osv_info in osv_infos {
            let version_odoo = odoo_version_u8_to_string(&(osv_info.version_odoo as u8));
            affected_modules
                .entry(version_odoo.clone())
                .or_default()
                .insert((
                    osv_info.org_name.clone(),
                    osv_info.module_technical_name.clone(),
                ));
            let by_ver = res.entry(version_odoo).or_default();
            let by_pack = by_ver.entry(osv_info.name).or_default();
            let entry = by_pack.entry(osv_info.osv_id.clone()).or_insert(OSVInfo {
                osv_id: osv_info.osv_id,
                details: osv_info.details,
                fixed_in: osv_info.fixed_in,
                modules: Vec::new(),
            });
            entry.modules.push(OSVModuleRef {
                org_name: osv_info.org_name,
                technical_name: osv_info.module_technical_name,
                name: osv_info.module_name,
            });
        }

        let mut summary: HashMap<String, OSVVersionSummary> = HashMap::new();
        for (version_odoo, by_pack) in &res {
            let vuln_count = by_pack.values().map(|by_id| by_id.len()).sum();
            summary.insert(
                version_odoo.clone(),
                OSVVersionSummary {
                    vuln_count,
                    package_count: by_pack.len(),
                    module_count: affected_modules
                        .get(version_odoo)
                        .map(|m| m.len())
                        .unwrap_or_default(),
                },
            );
        }

        // Flatten the per-osv_id map into a Vec for straightforward template iteration.
        let res: HashMap<String, HashMap<String, Vec<OSVInfo>>> = res
            .into_iter()
            .map(|(version_odoo, by_pack)| {
                let by_pack = by_pack
                    .into_iter()
                    .map(|(package_name, by_id)| (package_name, by_id.into_values().collect()))
                    .collect();
                (version_odoo, by_pack)
            })
            .collect();

        (res, summary)
    })
    .await?;

    tmpl_env.render(
        "pages/osv.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "osv",
                osv_info => res,
                osv_summary => summary,
            )
        ),
    )
}
