// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_author;

use super::{author, module, system_event};
use oghutils::version::odoo_version_u8_to_string;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_author, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub author_id: i64,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct TopAuthorJSON {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub author_id: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
}

#[derive(Insertable)]
#[diesel(table_name = module_author)]
struct NewModuleAuthor {
    module_id: i64,
    author_id: i64,
}

pub fn get_by_id(conn: &mut SqliteConnection, module_id: &i64, author_id: &i64) -> Option<Model> {
    module_author::table
        .filter(
            module_author::module_id
                .eq(module_id)
                .and(module_author::author_id.eq(author_id)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in module_author::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, module_id: &i64, name: &str) -> Option<Model> {
    if let Some(author) = author::get_by_name(conn, name) {
        module_author::table
            .filter(
                module_author::module_id
                    .eq(module_id)
                    .and(module_author::author_id.eq(author.id)),
            )
            .first::<Model>(conn)
            .optional()
            .expect("DB error in module_author::get_by_name")
    } else {
        None
    }
}

pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_author::table
        .filter(module_author::module_id.eq(module_id))
        .load::<Model>(conn)
        .expect("DB error in module_author::get_by_module_id")
}

pub fn get_names_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<String> {
    get_by_module_id(conn, module_id)
        .into_iter()
        .filter_map(|ma| author::get_by_id(conn, &ma.author_id).map(|a| a.name))
        .collect()
}

pub fn get_top_names(conn: &mut SqliteConnection, limit: &u8) -> Vec<TopAuthorJSON> {
    diesel::sql_query(
        "SELECT author_id, count(*) as count FROM module_author GROUP BY author_id ORDER BY count DESC LIMIT ?",
    )
    .bind::<diesel::sql_types::Integer, _>(*limit as i32)
    .load::<TopAuthorJSON>(conn)
    .expect("DB error in module_author::get_top_names")
}

pub fn add(conn: &mut SqliteConnection, module_id: &i64, name: &str) -> QueryResult<Model> {
    let author_rec = author::add(conn, name)?;
    if let Some(existing) = get_by_id(conn, module_id, &author_rec.id) {
        return Ok(existing);
    }

    diesel::insert_into(module_author::table)
        .values(NewModuleAuthor {
            module_id: *module_id,
            author_id: author_rec.id,
        })
        .execute(conn)?;
    let new_id = crate::models::last_insert_rowid(conn);
    let mod_info = module::get_by_id(conn, module_id).unwrap();
    let _ = system_event::register_new_module_author(
        conn,
        name,
        &mod_info.technical_name,
        &mod_info.name,
        odoo_version_u8_to_string(&(mod_info.version_odoo as u8)).as_str(),
    );
    Ok(Model {
        id: new_id,
        module_id: *module_id,
        author_id: author_rec.id,
    })
}

pub fn delete_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> QueryResult<usize> {
    diesel::delete(module_author::table.filter(module_author::module_id.eq(module_id)))
        .execute(conn)
}

pub fn delete_by_module_id_author_id(
    conn: &mut SqliteConnection,
    module_id: &i64,
    author_id: &i64,
) -> QueryResult<usize> {
    diesel::delete(
        module_author::table.filter(
            module_author::module_id
                .eq(module_id)
                .and(module_author::author_id.eq(author_id)),
        ),
    )
    .execute(conn)
}
