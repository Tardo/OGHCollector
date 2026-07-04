// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::pull_request;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = pull_request, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
    pub version_odoo: i32,
    pub module_technical_name: String,
    pub prid: i64,
    pub gh_repository_id: i64,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    pull_request::table
        .filter(pull_request::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in pull_request::get_by_id")
}
