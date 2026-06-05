// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::dependency_type;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = dependency_type, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
}

#[derive(Insertable)]
#[diesel(table_name = dependency_type)]
struct NewDependencyType<'a> {
    name: &'a str,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    dependency_type::table
        .filter(dependency_type::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in dependency_type::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, name: &str) -> Option<Model> {
    dependency_type::table
        .filter(dependency_type::name.eq(name))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in dependency_type::get_by_name")
}

pub fn get_by_name_no_cache(conn: &mut SqliteConnection, name: &str) -> Option<Model> {
    get_by_name(conn, name)
}
