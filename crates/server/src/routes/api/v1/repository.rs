// Copyright Alexandre D. Díaz
use std::collections::HashMap;

use actix_web::{get, web, Error as AWError, HttpResponse};
use serde::{Deserialize, Serialize};

use diesel::sqlite::SqliteConnection;
use oghutils::version::odoo_version_u8_to_string;
use sqlitedb::{models, Pool};

#[derive(Debug, Deserialize, Serialize)]
pub struct RepositoryGenericInfoResponse {
    pub name: String,
    pub organizations: HashMap<String, HashMap<String, u16>>,
}

fn get_repository_generic_info(
    conn: &mut SqliteConnection,
    repo_name: &str,
) -> Option<RepositoryGenericInfoResponse> {
    let repos = models::gh_repository::get_info_by_name(conn, repo_name);
    if repos.is_empty() {
        return None;
    }

    let mut orgs: HashMap<String, HashMap<String, u16>> = HashMap::new();
    for repo in repos {
        let branches = orgs.entry(repo.organization).or_default();
        branches
            .entry(odoo_version_u8_to_string(&(repo.version_odoo as u8)))
            .or_insert(repo.num_modules as u16);
    }
    Some(RepositoryGenericInfoResponse {
        name: repo_name.to_string(),
        organizations: orgs,
    })
}

#[get("/repo/{repo_name}")]
pub async fn route(
    pool: web::Data<Pool>,
    path: web::Path<String>,
) -> Result<HttpResponse, AWError> {
    let repo_name = path.into_inner();
    let result = web::block(move || {
        let mut conn = pool.get().unwrap();
        get_repository_generic_info(&mut conn, &repo_name)
    })
    .await?;
    Ok(HttpResponse::Ok().json(result))
}
