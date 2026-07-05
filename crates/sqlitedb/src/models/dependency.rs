// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::schema::dependency;

use super::module;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = dependency, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub dependency_type_id: i64,
    pub name: String,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct DependencyModuleInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub repo: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub module_id: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub module_name: String,
}

#[derive(Insertable)]
#[diesel(table_name = dependency)]
struct NewDependency<'a> {
    dependency_type_id: i64,
    name: &'a str,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    dependency::table
        .filter(dependency::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in dependency::get_by_id")
}

pub fn get_by_name(conn: &mut SqliteConnection, dep_type_id: &i64, name: &str) -> Option<Model> {
    dependency::table
        .filter(
            dependency::dependency_type_id
                .eq(dep_type_id)
                .and(dependency::name.eq(name)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in dependency::get_by_name")
}

pub fn get_module_external_dependency_names(
    conn: &mut SqliteConnection,
    module_id: &i64,
    dep_type: &str,
) -> Vec<String> {
    diesel::sql_query(
        "SELECT dep.name \
         FROM dependency as dep \
         INNER JOIN dependency_module as dep_mod ON dep_mod.dependency_id = dep.id \
         INNER JOIN dependency_type as dep_type ON dep_type.id = dep.dependency_type_id \
         INNER JOIN module as mod ON mod.id = dep_mod.module_id \
         WHERE mod.id = ? AND dep_type.name = ?",
    )
    .bind::<diesel::sql_types::BigInt, _>(module_id)
    .bind::<diesel::sql_types::Text, _>(dep_type)
    .load::<crate::models::NameRow>(conn)
    .expect("DB error in dependency::get_module_external_dependency_names")
    .into_iter()
    .map(|r| r.name)
    .collect()
}

pub fn get_module_dependency_info(
    conn: &mut SqliteConnection,
    module_id: &i64,
) -> Vec<DependencyModuleInfo> {
    diesel::sql_query(
        "SELECT ghorg.name as org, ghrepo.name as repo, mod_dep.id as module_id, dep.name as module_name \
         FROM dependency as dep \
         INNER JOIN dependency_module as dep_mod ON dep_mod.dependency_id = dep.id \
         INNER JOIN dependency_type as dep_type ON dep_type.id = dep.dependency_type_id \
         INNER JOIN module as mod ON mod.id = dep_mod.module_id \
         INNER JOIN module as mod_dep ON mod_dep.technical_name = dep.name AND mod_dep.version_odoo = mod.version_odoo \
         INNER JOIN gh_repository as ghrepo ON ghrepo.id = mod_dep.gh_repository_id \
         INNER JOIN gh_organization as ghorg ON ghorg.id = ghrepo.gh_organization_id \
         WHERE mod.id = ?",
    )
    .bind::<diesel::sql_types::BigInt, _>(module_id)
    .load::<DependencyModuleInfo>(conn)
    .expect("DB error in dependency::get_module_dependency_info")
}

/// Full (transitive) dependency closure of a module: every Odoo addon it
/// depends on directly or indirectly (grouped by `org/repo`), plus the
/// flattened, deduped external pip/bin dependencies of the whole closure.
#[derive(Debug, Default, Clone)]
pub struct FullDependencyInfo {
    pub odoo: HashMap<String, Vec<String>>,
    pub pip: Vec<String>,
    pub bin: Vec<String>,
}

fn collect_full_dependency_info(
    conn: &mut SqliteConnection,
    mod_: &module::Model,
    info: &mut FullDependencyInfo,
) {
    info.pip.extend(get_module_external_dependency_names(
        conn, &mod_.id, "python",
    ));
    info.bin
        .extend(get_module_external_dependency_names(conn, &mod_.id, "bin"));
    for dep in get_module_dependency_info(conn, &mod_.id) {
        let repo_deps = info
            .odoo
            .entry(format!("{}/{}", &dep.org, &dep.repo))
            .or_default();
        if !repo_deps.contains(&dep.module_name) {
            repo_deps.push(dep.module_name.clone());
            let dep_module = module::get_by_id(conn, &dep.module_id)
                .expect("dependency module referenced by dependency table not found");
            collect_full_dependency_info(conn, &dep_module, info);
        }
    }
}

pub fn get_full_dependency_info(
    conn: &mut SqliteConnection,
    mod_: &module::Model,
) -> FullDependencyInfo {
    let mut info = FullDependencyInfo::default();
    collect_full_dependency_info(conn, mod_, &mut info);
    let mut seen = HashSet::new();
    info.pip.retain(|x| seen.insert(x.clone()));
    seen.clear();
    info.bin.retain(|x| seen.insert(x.clone()));
    info
}

pub fn add(conn: &mut SqliteConnection, dep_type_id: &i64, name: &str) -> QueryResult<Model> {
    let inserted = diesel::insert_into(dependency::table)
        .values(NewDependency {
            dependency_type_id: *dep_type_id,
            name,
        })
        .on_conflict((dependency::dependency_type_id, dependency::name))
        .do_nothing()
        .execute(conn)?;

    if inserted == 0 {
        dependency::table
            .filter(
                dependency::dependency_type_id
                    .eq(dep_type_id)
                    .and(dependency::name.eq(name)),
            )
            .first::<Model>(conn)
    } else {
        let id = crate::models::last_insert_rowid(conn);
        Ok(Model {
            id,
            dependency_type_id: *dep_type_id,
            name: name.to_string(),
        })
    }
}
