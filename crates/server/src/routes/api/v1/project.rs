// Copyright Alexandre D. Díaz
//! Doodba projects as JSON, behind `projects_enabled` like their pages: `/v1/projects` lists
//! them (`AreaPool`; `org`, `odoo_version`, `module` filters) and
//! `/v1/project/{org}/{repository}/{odoo_version}` answers one whole (`web::Data<Pool>`: its
//! addons resolve in every workspace). The answers are `doodba_project::Summary`/`Report`, the
//! same views MCP answers with.
use actix_web::{get, web, Error as AWError, HttpResponse};
use serde::Deserialize;

use sqlitedb::models::doodba_project;
use sqlitedb::Pool;

use crate::workspace::AreaPool;

#[derive(Debug, Deserialize)]
pub struct ProjectsRequest {
    org: Option<String>,
    odoo_version: Option<String>,
    /// A technical name: only the projects that enable it.
    module: Option<String>,
}

#[get("/projects")]
pub async fn route(
    pool: AreaPool,
    info: web::Query<ProjectsRequest>,
) -> Result<HttpResponse, AWError> {
    let info = info.into_inner();
    let version_odoo = info
        .odoo_version
        .as_deref()
        .map(crate::utils::odoo_version)
        .transpose()?;
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        let rows = match &info.module {
            Some(module) => doodba_project::using_module(&mut conn, module, None),
            None => doodba_project::list(&mut conn),
        };
        rows.iter()
            .filter(|r| info.org.as_ref().is_none_or(|org| *org == r.org_name))
            .filter(|r| version_odoo.is_none_or(|v| v as i32 == r.project.version_odoo))
            .map(|r| r.summary())
            .collect::<Vec<_>>()
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/project/{org}/{repository}/{odoo_version}")]
pub async fn route_project(
    pool: web::Data<Pool>,
    path: web::Path<(String, String, String)>,
) -> Result<HttpResponse, AWError> {
    let (org, repository, odoo_version) = path.into_inner();
    let version_odoo = crate::utils::odoo_version(&odoo_version)? as i32;
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        let row = doodba_project::get(&mut conn, &org, &repository, version_odoo)?;
        Some(doodba_project::report(&mut conn, &row))
    })
    .await?;
    Ok(match result {
        Some(report) => HttpResponse::Ok().json(report),
        None => {
            HttpResponse::NotFound().json(serde_json::json!({"error": "No such Doodba project"}))
        }
    })
}
