// Copyright Alexandre D. Díaz
use actix_web::{get, web, HttpRequest, Responder, Result};
use minijinja::context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

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

// One Odoo-version tab's worth of content, ordered newest-first like the
// rest of the version-tabbed pages (see modules.rs).
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct OSVVersionGroup {
    pub odoo_version: String,
    pub packages: HashMap<String, Vec<OSVInfo>>,
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
    let res = web::block(move || {
        let mut conn = pool.get().unwrap();
        let osv_infos = models::dependency_osv::get_osv_info(&mut conn);
        // Group package-first (not module-first): the same vulnerability is
        // shared by every module depending on the vulnerable package, so
        // grouping by module repeats each vuln once per affected module.
        let mut res: BTreeMap<i32, HashMap<String, HashMap<String, OSVInfo>>> = BTreeMap::new();
        let mut affected_modules: HashMap<i32, HashSet<(String, String)>> = HashMap::new();
        for osv_info in osv_infos {
            let version_odoo = osv_info.version_odoo;
            affected_modules.entry(version_odoo).or_default().insert((
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

        // Newest Odoo version first.
        res.into_iter()
            .rev()
            .map(|(version_odoo, by_pack)| {
                let vuln_count = by_pack.values().map(|by_id| by_id.len()).sum();
                let package_count = by_pack.len();
                let module_count = affected_modules
                    .get(&version_odoo)
                    .map(|m| m.len())
                    .unwrap_or_default();
                let packages = by_pack
                    .into_iter()
                    .map(|(package_name, by_id)| (package_name, by_id.into_values().collect()))
                    .collect();
                OSVVersionGroup {
                    odoo_version: odoo_version_u8_to_string(&(version_odoo as u8)),
                    packages,
                    vuln_count,
                    package_count,
                    module_count,
                }
            })
            .collect::<Vec<_>>()
    })
    .await?;

    tmpl_env.render(
        "pages/osv.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "osv",
                osv_info => res,
            )
        ),
    )
}
