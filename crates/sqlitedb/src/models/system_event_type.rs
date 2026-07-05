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

/// Looks up an event type by name, creating it on first use. Event types used
/// to be a closed, pre-seeded set (see the migration), which meant logging a
/// genuinely new kind of action required a seed migration first. Now any
/// caller can introduce a new type just by naming it.
pub fn get_or_create(conn: &mut SqliteConnection, name: &str) -> Model {
    let inserted = diesel::insert_into(system_event_type::table)
        .values(NewSystemEventType { name })
        .on_conflict(system_event_type::name)
        .do_nothing()
        .execute(conn)
        .expect("DB error in system_event_type::get_or_create");

    if inserted == 0 {
        get_by_name(conn, name).expect("system_event_type must exist after get_or_create")
    } else {
        Model {
            id: crate::models::last_insert_rowid(conn),
            name: name.to_string(),
        }
    }
}
