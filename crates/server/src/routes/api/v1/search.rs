// Copyright Alexandre D. Díaz
use std::collections::HashMap;

use actix_web::{get, web, Error as AWError, HttpResponse};
use serde::{Deserialize, Serialize};

use diesel::sqlite::SqliteConnection;
use oghutils::version::{odoo_version_string_to_u8, odoo_version_u8_to_string};
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchGenericInfoResponse {
    pub technical_name: String,
    pub versions: HashMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct RouteSearchRequest {
    odoo_version: Option<String>,
    installable: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct RouteSemanticSearchRequest {
    q: String,
    odoo_version: Option<String>,
    limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct SemanticSearchResponse {
    pub technical_name: String,
    pub name: String,
    pub category: String,
    pub odoo_version: String,
    pub org_name: String,
    pub repository: String,
    /// Hybrid relevance (roughly 0..1, higher is closer): cosine similarity
    /// query vs the module's embedded name/category/description, boosted
    /// when query words appear verbatim in the module's names.
    pub score: f32,
}

#[derive(Debug, Deserialize)]
pub struct RouteSearchCriteriaRequest {
    odoo_version: String,
    term: Option<String>,
    category: Option<String>,
    depends_on: Option<String>,
    limit: Option<u32>,
}

fn build_response(
    modules: Vec<sqlitedb::models::module::ModuleGenericInfo>,
) -> Vec<SearchGenericInfoResponse> {
    let mut res = Vec::new();
    for module in modules {
        let src_versions: Vec<&str> = module.versions.split(',').collect();
        let mut srcs: HashMap<String, Vec<String>> = HashMap::new();
        let versions = srcs.entry(module.src).or_default();
        let mut frmt = src_versions
            .iter()
            .filter_map(|&x| x.trim().parse::<u8>().ok())
            .map(|v| odoo_version_u8_to_string(&v))
            .collect::<Vec<String>>();
        versions.append(&mut frmt);
        res.push(SearchGenericInfoResponse {
            technical_name: module.technical_name,
            versions: srcs,
        });
    }
    res
}

fn get_modules(conn: &mut SqliteConnection, module_name: &str) -> Vec<SearchGenericInfoResponse> {
    build_response(models::module::get_generic_info(conn, module_name))
}

fn get_modules_by_odoo_version_installable(
    conn: &mut SqliteConnection,
    module_name: &str,
    odoo_version: &u8,
    installable: &bool,
) -> Vec<SearchGenericInfoResponse> {
    build_response(
        models::module::get_generic_info_by_odoo_version_installable(
            conn,
            module_name,
            odoo_version,
            installable,
        ),
    )
}

fn get_modules_by_odoo_version(
    conn: &mut SqliteConnection,
    module_name: &str,
    odoo_version: &u8,
) -> Vec<SearchGenericInfoResponse> {
    build_response(models::module::get_generic_info_by_odoo_version(
        conn,
        module_name,
        odoo_version,
    ))
}

fn get_modules_by_installable(
    conn: &mut SqliteConnection,
    module_name: &str,
    installable: &bool,
) -> Vec<SearchGenericInfoResponse> {
    build_response(models::module::get_generic_info_by_installable(
        conn,
        module_name,
        installable,
    ))
}

/// Embedding-based free-text search: embeds `q` (any language) and ranks it
/// against every collector-built module vector, blended with an IDF-weighted
/// verbatim-keyword boost so rare product names still hit - same approach
/// (and shared `oghembed::top_k` ranking) as the MCP
/// `semantic_search_modules` tool. One row per module row, so a module
/// carried at several Odoo versions comes back once per version; best
/// score first. Own top-level path (not /search/semantic) so it can never
/// collide with the /search/{module_name} dynamic segment.
#[get("/semantic-search")]
pub async fn route_semantic(
    pool: web::Data<Pool>,
    info: web::Query<RouteSemanticSearchRequest>,
) -> Result<HttpResponse, AWError> {
    let params = info.into_inner();
    let limit = params.limit.unwrap_or(20).min(50) as usize;
    let result = web::block(move || -> Result<Vec<SemanticSearchResponse>, String> {
        let query_vec = oghembed::embed_texts(&[params.q.as_str()])
            .map_err(|e| format!("embedding model unavailable: {e}"))?
            .pop()
            .ok_or("embedding model returned no vector")?;
        let mut conn = pool.get().unwrap();
        let version_filter = params
            .odoo_version
            .as_deref()
            .map(odoo_version_string_to_u8);
        let rows = models::module_embedding::get_vectors(&mut conn, version_filter.as_ref());
        let lexical =
            models::module_embedding::lexical_scores(&mut conn, &params.q, version_filter.as_ref());
        let scored = oghembed::top_k(
            &query_vec,
            rows.iter().map(|r| (r.module_id, r.embedding.as_slice())),
            &lexical,
            limit,
        );
        Ok(scored
            .into_iter()
            .filter_map(|(module_id, score)| {
                let module = models::module::get_by_id(&mut conn, &module_id)?;
                let repo = models::gh_repository::get_by_id(&mut conn, &module.gh_repository_id)?;
                let org = models::gh_organization::get_by_id(&mut conn, &repo.gh_organization_id)?;
                Some(SemanticSearchResponse {
                    technical_name: module.technical_name,
                    name: module.name,
                    category: module.category.unwrap_or_default(),
                    odoo_version: odoo_version_u8_to_string(&(module.version_odoo as u8)),
                    org_name: org.name,
                    repository: repo.name,
                    score,
                })
            })
            .collect())
    })
    .await?
    .map_err(actix_web::error::ErrorInternalServerError)?;
    Ok(HttpResponse::Ok().json(result))
}

/// Cross-repository discovery by free-text term, category and/or reverse
/// Odoo dependency ("which modules depend on X"); same query (and limit cap)
/// as the MCP `list_modules_by_criteria` tool.
#[get("/search")]
pub async fn route_criteria(
    pool: web::Data<Pool>,
    info: web::Query<RouteSearchCriteriaRequest>,
) -> Result<HttpResponse, AWError> {
    let params = info.into_inner();
    let version_odoo = odoo_version_string_to_u8(&params.odoo_version);
    let limit = params.limit.unwrap_or(50).min(200) as i64;
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        models::module::search_by_criteria(
            &mut conn,
            &version_odoo,
            params.term.as_deref(),
            params.category.as_deref(),
            params.depends_on.as_deref(),
            limit,
        )
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}

#[get("/search/{module_name}")]
pub async fn route(
    pool: web::Data<Pool>,
    path: web::Path<String>,
    info: web::Query<RouteSearchRequest>,
) -> Result<HttpResponse, AWError> {
    let module_name = path.into_inner();
    if let (Some(version_odoo), Some(installable)) = (info.odoo_version.clone(), info.installable) {
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            get_modules_by_odoo_version_installable(
                &mut conn,
                &module_name,
                &odoo_version_string_to_u8(&version_odoo),
                &installable,
            )
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if info.odoo_version.is_some() {
        let version_odoo = info.odoo_version.clone().unwrap();
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            get_modules_by_odoo_version(
                &mut conn,
                &module_name,
                &odoo_version_string_to_u8(&version_odoo),
            )
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    } else if let Some(installable) = info.installable {
        let result = web::block(move || {
            let mut conn = pool.get().unwrap();
            get_modules_by_installable(&mut conn, &module_name, &installable)
        })
        .await?;
        return Ok(HttpResponse::Ok().json(result));
    }
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_modules(&mut conn, &module_name)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
