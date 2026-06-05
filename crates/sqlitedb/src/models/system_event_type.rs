// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::system_event_type;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = system_event_type, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
}

#[derive(Insertable)]
#[diesel(table_name = system_event_type)]
struct NewSystemEventType<'a> {
    name: &'a str,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    system_event_type::table
        .filter(system_event_type::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in system_event_type::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, name: &str) -> Option<Model> {
    system_event_type::table
        .filter(system_event_type::name.eq(name))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in system_event_type::get_by_name")
}
