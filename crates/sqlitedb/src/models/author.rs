// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::author;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = author, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub name: String,
}

#[derive(Insertable)]
#[diesel(table_name = author)]
struct NewAuthor<'a> {
    name: &'a str,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    author::table
        .filter(author::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in author::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, name: &str) -> Option<Model> {
    author::table
        .filter(author::name.eq(name))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in author::get_by_name")
}

pub fn add(conn: &mut SqliteConnection, name: &str) -> QueryResult<Model> {
    let inserted = diesel::insert_into(author::table)
        .values(NewAuthor { name })
        .on_conflict(author::name)
        .do_nothing()
        .execute(conn)?;

    if inserted == 0 {
        author::table
            .filter(author::name.eq(name))
            .first::<Model>(conn)
    } else {
        let id = crate::models::last_insert_rowid(conn);
        Ok(Model {
            id,
            name: name.to_string(),
        })
    }
}
