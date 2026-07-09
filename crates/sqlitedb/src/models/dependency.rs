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
    visited: &mut HashSet<i64>,
) {
    info.pip.extend(get_module_external_dependency_names(
        conn, &mod_.id, "python",
    ));
    info.bin
        .extend(get_module_external_dependency_names(conn, &mod_.id, "bin"));
    for dep in get_module_dependency_info(conn, &mod_.id) {
        let repo_deps = info
            .odoo
            .entry(format!("{}/{}", dep.org, dep.repo))
            .or_default();
        if !repo_deps.contains(&dep.module_name) {
            repo_deps.push(dep.module_name.clone());
        }
        // Recurse by module id, not by the (repo, name) grouping above: a
        // dependency cycle routes back to a module id already on the
        // current path (the root's own id is pre-seeded below), so this
        // must stop it even on the first repeat visit or it recurses forever.
        if visited.insert(dep.module_id) {
            let dep_module = module::get_by_id(conn, &dep.module_id)
                .expect("dependency module referenced by dependency table not found");
            collect_full_dependency_info(conn, &dep_module, info, visited);
        }
    }
}

pub fn get_full_dependency_info(
    conn: &mut SqliteConnection,
    mod_: &module::Model,
) -> FullDependencyInfo {
    let mut info = FullDependencyInfo::default();
    let mut visited = HashSet::from([mod_.id]);
    collect_full_dependency_info(conn, mod_, &mut info, &mut visited);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{dependency_module, dependency_type, module};
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};
    use std::collections::HashMap;

    const MIGRATIONS: EmbeddedMigrations = embed_migrations!("../../migrations");

    fn setup_db() -> SqliteConnection {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.run_pending_migrations(MIGRATIONS).unwrap();
        conn
    }

    fn make_module(
        conn: &mut SqliteConnection,
        tech_name: &str,
        git_org: &str,
        git_repo: &str,
        version_odoo: u8,
    ) -> module::Model {
        module::add(
            conn,
            &module::ManifestInfo {
                technical_name: tech_name.to_string(),
                version_odoo,
                name: tech_name.to_string(),
                version_module: "1.0.0".to_string(),
                description: String::new(),
                installation: String::new(),
                usage: String::new(),
                author: String::new(),
                website: String::new(),
                license: String::new(),
                category: String::new(),
                auto_install: false,
                application: false,
                installable: true,
                maintainer: String::new(),
                git_org: git_org.to_string(),
                git_repo: git_repo.to_string(),
                depends: vec![],
                external_depends_python: vec![],
                external_depends_bin: vec![],
                folder_size: 0,
                last_commit_hash: "abc".to_string(),
                last_commit_author: String::new(),
                last_commit_date: "2024-01-01".to_string(),
                last_commit_name: String::new(),
                last_commit_partof: String::new(),
                committers: HashMap::new(),
                analysis: Default::default(),
            },
        )
        .unwrap()
    }

    fn link_module_dep(conn: &mut SqliteConnection, from: &module::Model, to: &module::Model) {
        let dep_type = dependency_type::get_by_name(conn, "module").unwrap();
        dependency_module::add(conn, &dep_type.id, &to.technical_name, &from.id).unwrap();
    }

    fn link_external_dep(
        conn: &mut SqliteConnection,
        dep_type: &str,
        name: &str,
        on: &module::Model,
    ) {
        let dep_type = dependency_type::get_by_name(conn, dep_type).unwrap();
        dependency_module::add(conn, &dep_type.id, name, &on.id).unwrap();
    }

    // Diamond graph: root -> mid1, mid2; mid1 -> leaf; mid2 -> leaf. Also
    // covers grouping two modules from the same repo under one key, and
    // deduplicating a repo (leaf) reached through two different paths.
    #[test]
    fn test_full_dependency_info_diamond_graph_is_deduped_and_grouped_by_repo() {
        let mut conn = setup_db();
        let root = make_module(&mut conn, "root", "OrgA", "repo-root", 16);
        let mid1 = make_module(&mut conn, "mid1", "OrgA", "repo-mid", 16);
        let mid2 = make_module(&mut conn, "mid2", "OrgA", "repo-mid", 16);
        let leaf = make_module(&mut conn, "leaf", "OrgB", "repo-leaf", 16);

        link_module_dep(&mut conn, &root, &mid1);
        link_module_dep(&mut conn, &root, &mid2);
        link_module_dep(&mut conn, &mid1, &leaf);
        link_module_dep(&mut conn, &mid2, &leaf);

        link_external_dep(&mut conn, "python", "requests", &leaf);
        link_external_dep(&mut conn, "python", "requests", &mid1); // duplicate on purpose
        link_external_dep(&mut conn, "bin", "graphviz", &mid2);
        link_external_dep(&mut conn, "bin", "libxml2", &leaf);

        let info = get_full_dependency_info(&mut conn, &root);

        let mut mid_repo = info.odoo.get("OrgA/repo-mid").cloned().unwrap();
        mid_repo.sort();
        assert_eq!(mid_repo, vec!["mid1", "mid2"]);

        // leaf reached via both mid1 and mid2, but must appear once.
        assert_eq!(
            info.odoo.get("OrgB/repo-leaf").cloned().unwrap(),
            vec!["leaf"]
        );

        assert_eq!(info.pip, vec!["requests"]);
        let mut bin = info.bin.clone();
        bin.sort();
        assert_eq!(bin, vec!["graphviz", "libxml2"]);
    }

    #[test]
    fn test_full_dependency_info_handles_cycle_without_infinite_recursion() {
        let mut conn = setup_db();
        let root = make_module(&mut conn, "root", "OrgA", "repo-root", 16);
        let mid = make_module(&mut conn, "mid", "OrgA", "repo-mid", 16);
        link_module_dep(&mut conn, &root, &mid);
        link_module_dep(&mut conn, &mid, &root); // cycle back to the entry point

        let info = get_full_dependency_info(&mut conn, &root);

        assert_eq!(
            info.odoo.get("OrgA/repo-mid").cloned().unwrap(),
            vec!["mid"]
        );
        assert_eq!(
            info.odoo.get("OrgA/repo-root").cloned().unwrap(),
            vec!["root"]
        );
    }

    #[test]
    fn test_full_dependency_info_leaf_module_has_no_odoo_deps() {
        let mut conn = setup_db();
        let leaf = make_module(&mut conn, "leaf", "OrgB", "repo-leaf", 16);
        link_external_dep(&mut conn, "python", "requests", &leaf);

        let info = get_full_dependency_info(&mut conn, &leaf);
        assert!(info.odoo.is_empty());
        assert_eq!(info.pip, vec!["requests"]);
        assert!(info.bin.is_empty());
    }
}
