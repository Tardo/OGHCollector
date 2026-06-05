// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::maintainer;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = maintainer, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
}

#[derive(Insertable)]
#[diesel(table_name = maintainer)]
struct NewMaintainer<'a> {
    name: &'a str,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    maintainer::table
        .filter(maintainer::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in maintainer::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, name: &str) -> Option<Model> {
    maintainer::table
        .filter(maintainer::name.eq(name))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in maintainer::get_by_name")
}

pub fn add(conn: &mut SqliteConnection, name: &str) -> QueryResult<Model> {
    let inserted = diesel::insert_into(maintainer::table)
        .values(NewMaintainer { name })
        .on_conflict(maintainer::name)
        .do_nothing()
        .execute(conn)?;

    if inserted == 0 {
        maintainer::table
            .filter(maintainer::name.eq(name))
            .first::<Model>(conn)
    } else {
        let id = crate::models::last_insert_rowid(conn);
        Ok(Model {
            id,
            name: name.to_string(),
        })
    }
}
