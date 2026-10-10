// Copyright Alexandre D. Díaz
use actix_web::{get, post, web, Error as AWError, HttpRequest, HttpResponse, Responder, Result};
use diesel::sqlite::SqliteConnection;
use minijinja::context;
use oghutils::license::LicenseReport;
use oghutils::version::{odoo_version_u8_to_string, parse_odoo_version};
use serde::{Deserialize, Serialize};

use crate::minijinja_renderer::MiniJinjaRenderer;
use crate::routes::doodba_tools::validate_module_count;
use crate::utils::get_minijinja_context;
use sqlitedb::{models, Pool};

// Page shell only: the pack itself (name + module list) is encoded in the
// `?d=` URL param by the browser (see web/js/utils/favorites-store.mjs) and
// decoded client-side, same as /favorites never touching the DB for pack
// data. This route just serves the static page; pack.mjs POSTs the decoded
// module list to route_info below to resolve display stats.
#[get("/pack")]
pub async fn route(tmpl_env: MiniJinjaRenderer, req: HttpRequest) -> Result<impl Responder> {
    tmpl_env.render(
        "pages/pack.html",
        context!(
            ..get_minijinja_context(&req),
            ..context!(
                page_name => "pack"
            )
        ),
    )
}

#[derive(Debug, Deserialize)]
pub struct PackModuleRef {
    pub org: String,
    pub technical_name: String,
}

#[derive(Debug, Serialize)]
pub struct PackModuleInfoResponse {
    pub org: String,
    pub technical_name: String,
    pub found: bool,
    pub name: Option<String>,
    pub repository: Option<String>,
    /// The Odoo version this row was resolved against: `target_version` if
    /// given, else the most recently tracked one for this module.
    pub odoo_version: Option<String>,
    pub folder_size: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct PackInfoResponse {
    pub modules: Vec<PackModuleInfoResponse>,
    /// License review of every resolved module plus its dependency closure.
    pub license_review: LicenseReport,
}

// Deliberately not process_modules_db: that also computes code analysis and
// up to 500 required_by rows per module, which a pack of dozens of modules
// would multiply badly. A pack page only needs each module's own size at one
// version, plus one closure walk over the whole pack for the license review.
fn get_pack_module_info(
    conn: &mut SqliteConnection,
    refs: &[PackModuleRef],
    target_version: Option<u8>,
) -> PackInfoResponse {
    let mut resolved_modules = Vec::new();
    let modules = refs
        .iter()
        .map(|r| {
            let candidates = models::module::get_by_technical_name_organization_name(
                conn,
                &r.technical_name,
                &r.org,
            );
            let resolved = match target_version {
                Some(v) => candidates.into_iter().find(|m| m.version_odoo as u8 == v),
                None => candidates.into_iter().max_by_key(|m| m.version_odoo),
            };
            match resolved {
                Some(m) => {
                    let repository = models::gh_repository::get_by_id(conn, &m.gh_repository_id)
                        .map(|repo| repo.name);
                    let info = PackModuleInfoResponse {
                        org: r.org.clone(),
                        technical_name: r.technical_name.clone(),
                        found: true,
                        name: Some(m.name.clone()),
                        repository,
                        odoo_version: Some(odoo_version_u8_to_string(&(m.version_odoo as u8))),
                        folder_size: Some(m.folder_size as u64),
                    };
                    resolved_modules.push(m);
                    info
                }
                None => PackModuleInfoResponse {
                    org: r.org.clone(),
                    technical_name: r.technical_name.clone(),
                    found: false,
                    name: None,
                    repository: None,
                    odoo_version: None,
                    folder_size: None,
                },
            }
        })
        .collect();
    let root_ids: Vec<i64> = resolved_modules.iter().map(|m| m.id).collect();
    let license_review = models::dependency::get_full_dependency_info_many(conn, &resolved_modules)
        .license_review(&root_ids);
    PackInfoResponse {
        modules,
        license_review,
    }
}

#[derive(Debug, Deserialize)]
pub struct PackInfoQuery {
    /// Resolve every module against this Odoo version instead of each
    /// module's most recently tracked one - used when "converting" a pack
    /// to a different version on the /pack page.
    pub odoo_version: Option<String>,
}

#[post("/pack/info")]
pub async fn route_info(
    pool: web::Data<Pool>,
    query: web::Query<PackInfoQuery>,
    refs: web::Json<Vec<PackModuleRef>>,
) -> Result<HttpResponse, AWError> {
    validate_module_count(refs.len())?;
    let target_version = match query.into_inner().odoo_version {
        Some(v) => Some(parse_odoo_version(&v).ok_or_else(|| {
            actix_web::error::ErrorBadRequest(format!("invalid odoo version: {v:?}"))
        })?),
        None => None,
    };
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_pack_module_info(&mut conn, &refs, target_version)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
