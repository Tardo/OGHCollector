// Copyright Alexandre D. Díaz
//! License review (`oghutils::license` over the closure graph of `sqlitedb::models::dependency`):
//! `license_review` field on `/v1/module/*` (module page: License stat card, alert and facts row),
//! on the dependency resolver response and on `/pack/info` (now `{modules, license_review}`), plus
//! `GET /v1/license-check?odoo_version=&modules=a,b` for CI. "Combined: X" is shown only when
//! there's a `LICENSE_EFFECTIVE_UPGRADE` finding, since a proprietary module isn't distributed
//! under its dependencies' copyleft. `doodba_tools::validate_module_count` (`doodba_max_modules`)
//! caps every endpoint that walks the closures of a caller-supplied list.
use actix_web::{get, web, Error as AWError, HttpResponse};
use serde::{Deserialize, Serialize};

use oghutils::license::LicenseReport;
use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

use crate::routes::doodba_tools::validate_module_count;

#[derive(Debug, Deserialize)]
pub struct RouteLicenseCheckRequest {
    odoo_version: String,
    /// Comma-separated technical names.
    modules: String,
}

#[derive(Debug, Serialize)]
pub struct LicenseCheckResponse {
    pub odoo_version: String,
    /// Requested modules with no row at that Odoo version: not reviewed.
    pub not_found: Vec<String>,
    #[serde(flatten)]
    pub review: LicenseReport,
}

/// License review of a module set plus everything it depends on - e.g. a
/// project's addons list checked in CI, failing on any "error" finding.
#[get("/license-check")]
pub async fn route(
    pool: web::Data<Pool>,
    info: web::Query<RouteLicenseCheckRequest>,
) -> Result<HttpResponse, AWError> {
    let info = info.into_inner();
    let version_odoo = crate::utils::odoo_version(&info.odoo_version)?;
    let mut names: Vec<String> = info
        .modules
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect();
    names.sort_unstable();
    names.dedup();
    validate_module_count(names.len())?;

    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        let modules =
            models::module::get_by_technical_name_odoo_version(&mut conn, &names, &version_odoo);
        let not_found = names
            .into_iter()
            .filter(|name| !modules.iter().any(|m| &m.technical_name == name))
            .collect();
        let root_ids: Vec<i64> = modules.iter().map(|m| m.id).collect();
        let review = models::dependency::get_full_dependency_info_many(&mut conn, &modules)
            .license_review(&root_ids);
        LicenseCheckResponse {
            odoo_version: odoo_version_u8_to_string(&version_odoo),
            not_found,
            review,
        }
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
