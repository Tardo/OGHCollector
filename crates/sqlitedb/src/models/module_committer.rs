// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_committer;

use super::{committer, module, system_event};
use oghutils::version::odoo_version_u8_to_string;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_committer, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub committer_id: i64,
    pub commits: i32,
}

#[derive(Insertable)]
#[diesel(table_name = module_committer)]
struct NewModuleCommitter {
    module_id: i64,
    committer_id: i64,
    commits: i32,
}

pub fn get_by_id(
    conn: &mut SqliteConnection,
    module_id: &i64,
    committer_id: &i64,
) -> Option<Model> {
    module_committer::table
        .filter(
            module_committer::module_id
                .eq(module_id)
                .and(module_committer::committer_id.eq(committer_id)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in module_committer::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, module_id: &i64, name: &str) -> Option<Model> {
    if let Some(com) = committer::get_by_name(conn, name) {
        module_committer::table
            .filter(
                module_committer::module_id
                    .eq(module_id)
                    .and(module_committer::committer_id.eq(com.id)),
            )
            .first::<Model>(conn)
            .optional()
            .expect("DB error in module_committer::get_by_name")
    } else {
        None
    }
}

pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_committer::table
        .filter(module_committer::module_id.eq(module_id))
        .load::<Model>(conn)
        .expect("DB error in module_committer::get_by_module_id")
}

pub fn get_names_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<String> {
    get_by_module_id(conn, module_id)
        .into_iter()
        .filter_map(|mc| committer::get_by_id(conn, &mc.committer_id).map(|c| c.name))
        .collect()
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct CommitterModuleActivity {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub organization: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub repository: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub commits: i32,
}

pub fn get_activity_by_committer_name(
    conn: &mut SqliteConnection,
    committer_name: &str,
) -> Vec<CommitterModuleActivity> {
    diesel::sql_query(
        "SELECT mod.technical_name, mod.name, mod.version_odoo, \
         gh_org.name as organization, gh_repo.name as repository, mod_com.commits as commits \
         FROM module_committer as mod_com \
         INNER JOIN committer as com ON mod_com.committer_id = com.id \
         INNER JOIN module as mod ON mod_com.module_id = mod.id \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         WHERE com.name = ? \
         ORDER BY mod.version_odoo DESC, mod_com.commits DESC",
    )
    .bind::<diesel::sql_types::Text, _>(committer_name)
    .load::<CommitterModuleActivity>(conn)
    .expect("DB error in module_committer::get_activity_by_committer_name")
}

pub fn add(
    conn: &mut SqliteConnection,
    module_id: &i64,
    name: &str,
    commits: &u32,
) -> QueryResult<Model> {
    let com = committer::add(conn, name)?;
    let commits_i32 = *commits as i32;

    if let Some(existing) = get_by_id(conn, module_id, &com.id) {
        if existing.commits != commits_i32 {
            diesel::update(
                module_committer::table.filter(
                    module_committer::module_id
                        .eq(module_id)
                        .and(module_committer::committer_id.eq(com.id)),
                ),
            )
            .set(module_committer::commits.eq(commits_i32))
            .execute(conn)?;
        }
        return Ok(Model {
            id: existing.id,
            module_id: existing.module_id,
            committer_id: existing.committer_id,
            commits: commits_i32,
        });
    }

    diesel::insert_into(module_committer::table)
        .values(NewModuleCommitter {
            module_id: *module_id,
            committer_id: com.id,
            commits: commits_i32,
        })
        .execute(conn)?;
    let new_id = crate::models::last_insert_rowid(conn);
    let mod_info = module::get_by_id(conn, module_id).unwrap();
    let _ = system_event::register_new_module_committer(
        conn,
        name,
        &mod_info.technical_name,
        &mod_info.name,
        odoo_version_u8_to_string(&(mod_info.version_odoo as u8)).as_str(),
    );
    Ok(Model {
        id: new_id,
        module_id: *module_id,
        committer_id: com.id,
        commits: commits_i32,
    })
}
