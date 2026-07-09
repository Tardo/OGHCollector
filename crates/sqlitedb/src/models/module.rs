// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::schema::module;
use crate::utils::date::get_sqlite_utc_now;

use super::{
    author, gh_organization, gh_repository, maintainer, module_author,
    module_code_analysis::ModuleAnalysisInfo, module_committer, module_committer_period,
    module_maintainer, module_model, module_record, module_version, module_view, system_event,
    BOT_COMMITTERS,
};
use oghutils::version::odoo_version_u8_to_string;

use super::system_event::LogUpdateModuleInfo;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub technical_name: String,
    pub version_odoo: i32,
    pub name: String,
    pub version_module: String,
    pub description: Option<String>,
    pub website: Option<String>,
    pub license: Option<String>,
    pub category: Option<String>,
    pub auto_install: bool,
    pub application: bool,
    pub installable: bool,
    pub gh_repository_id: i64,
    pub create_date: String,
    pub update_date: String,
    pub folder_size: i64,
    pub last_commit_hash: String,
    pub last_commit_author: String,
    pub last_commit_name: String,
    pub last_commit_date: String,
    pub last_commit_partof: Option<String>,
    pub installation: Option<String>,
    pub usage: Option<String>,
}

impl Model {
    pub fn description_str(&self) -> &str {
        self.description.as_deref().unwrap_or("")
    }
    pub fn website_str(&self) -> &str {
        self.website.as_deref().unwrap_or("")
    }
    pub fn license_str(&self) -> &str {
        self.license.as_deref().unwrap_or("LGPL-3")
    }
    pub fn category_str(&self) -> &str {
        self.category.as_deref().unwrap_or("Uncategorized")
    }
    pub fn last_commit_partof_str(&self) -> &str {
        self.last_commit_partof.as_deref().unwrap_or("")
    }
    pub fn installation_str(&self) -> &str {
        self.installation.as_deref().unwrap_or("")
    }
    pub fn usage_str(&self) -> &str {
        self.usage.as_deref().unwrap_or("")
    }
}

/// A committer's activity within a module: total commit count plus a
/// (year, month) -> commits breakdown, both derived from the same `git log`
/// run so they always agree (see module_committer vs module_committer_period).
#[derive(Clone, Default)]
pub struct CommitterActivity {
    pub total: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub periods: HashMap<(i32, i32), u32>,
}

#[derive(Clone)]
pub struct ManifestInfo {
    pub technical_name: String,
    pub version_odoo: u8,
    pub name: String,
    pub version_module: String,
    pub description: String,
    pub installation: String,
    pub usage: String,
    pub author: String,
    pub website: String,
    pub license: String,
    pub category: String,
    pub auto_install: bool,
    pub application: bool,
    pub installable: bool,
    pub maintainer: String,
    pub git_org: String,
    pub git_repo: String,
    pub depends: Vec<String>,
    pub external_depends_python: Vec<String>,
    pub external_depends_bin: Vec<String>,
    pub folder_size: u64,
    pub last_commit_hash: String,
    pub last_commit_author: String,
    pub last_commit_date: String,
    pub last_commit_name: String,
    pub last_commit_partof: String,
    pub committers: HashMap<String, CommitterActivity>,
    pub analysis: ModuleAnalysisInfo,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleInfo {
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
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleGenericInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub versions: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub src: String,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleCriteriaInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub name: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub category: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub installable: bool,
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub application: bool,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub organization: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub repository: String,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleCountInfo {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleDistinctCountInfo {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleCountByOrganizationInfo {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleRankContributorInfo {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub contrib_name: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub rank: i64,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleRankCommitterInfo {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub committer_name: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub rank: i64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleRepositoryInfo {
    pub technical_name: String,
    pub repository_name: String,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleLastCreatedInfo {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub id: i64,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub create_date: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleListInfo {
    pub technical_name: String,
    pub org_name: String,
    pub versions_odoo: Vec<i32>,
}

#[derive(QueryableByName)]
struct ModuleListRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    org_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    versions_str: String,
}

#[derive(Insertable)]
#[diesel(table_name = module)]
struct NewModule<'a> {
    technical_name: &'a str,
    version_odoo: i32,
    name: &'a str,
    version_module: &'a str,
    description: Option<&'a str>,
    installation: Option<&'a str>,
    usage: Option<&'a str>,
    website: Option<&'a str>,
    license: Option<&'a str>,
    category: Option<&'a str>,
    auto_install: bool,
    application: bool,
    installable: bool,
    gh_repository_id: i64,
    create_date: &'a str,
    update_date: &'a str,
    folder_size: i64,
    last_commit_hash: &'a str,
    last_commit_author: &'a str,
    last_commit_name: &'a str,
    last_commit_date: &'a str,
    last_commit_partof: Option<&'a str>,
}

pub fn get_by_id(conn: &mut SqliteConnection, id: &i64) -> Option<Model> {
    module::table
        .filter(module::id.eq(id))
        .first::<Model>(conn)
        .optional()
        .expect("DB error in module::get_by_id")
}

pub fn get_by_odoo_version(conn: &mut SqliteConnection, version_odoo: &u8) -> Vec<Model> {
    module::table
        .filter(module::version_odoo.eq(*version_odoo as i32))
        .load::<Model>(conn)
        .expect("DB error in module::get_by_odoo_version")
}

pub fn get_by_technical_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
    gh_repo_id: &i64,
) -> Option<Model> {
    module::table
        .filter(
            module::technical_name
                .eq(technical_name)
                .and(module::version_odoo.eq(*version_odoo as i32))
                .and(module::gh_repository_id.eq(gh_repo_id)),
        )
        .first::<Model>(conn)
        .optional()
        .expect("DB error in module::get_by_technical_name")
}

pub fn get_by_technical_name_odoo_version(
    conn: &mut SqliteConnection,
    modules: &[String],
    version_odoo: &u8,
) -> Vec<Model> {
    module::table
        .filter(
            module::technical_name
                .eq_any(modules)
                .and(module::version_odoo.eq(*version_odoo as i32)),
        )
        .load::<Model>(conn)
        .expect("DB error in module::get_by_technical_name_odoo_version")
}

pub fn get_by_technical_name_odoo_version_organization_name_repository_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
    org_name: &str,
    repo_name: &str,
) -> Vec<Model> {
    use crate::schema::{gh_organization, gh_repository};
    module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            module::technical_name
                .eq(technical_name)
                .and(module::version_odoo.eq(*version_odoo as i32))
                .and(gh_repository::name.eq(repo_name))
                .and(gh_organization::name.eq(org_name)),
        )
        .select(Model::as_select())
        .load::<Model>(conn)
        .expect("DB error in module::get_by_technical_name_odoo_version_organization_name_repository_name")
}

pub fn get_by_technical_name_odoo_version_organization_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
    org_name: &str,
) -> Vec<Model> {
    use crate::schema::{gh_organization, gh_repository};
    module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            module::technical_name
                .eq(technical_name)
                .and(module::version_odoo.eq(*version_odoo as i32))
                .and(gh_organization::name.eq(org_name)),
        )
        .select(Model::as_select())
        .load::<Model>(conn)
        .expect("DB error in module::get_by_technical_name_odoo_version_organization_name")
}

/// Every module recorded for one repository, across all Odoo versions and
/// module versions currently known. This is the bulk listing used to build a
/// module pack (e.g. every module OCA carries for a country's localization,
/// which is organized one repository per country) - filtering by
/// technical_name substring would miss modules that don't follow the naming
/// convention (e.g. `delivery_dhl_parcel` living in `OCA/l10n-spain`).
pub fn get_by_organization_repository_name(
    conn: &mut SqliteConnection,
    org_name: &str,
    repo_name: &str,
) -> Vec<Model> {
    use crate::schema::{gh_organization, gh_repository};
    module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            gh_organization::name
                .eq(org_name)
                .and(gh_repository::name.eq(repo_name)),
        )
        .select(Model::as_select())
        .load::<Model>(conn)
        .expect("DB error in module::get_by_organization_repository_name")
}

pub fn get_by_technical_name_organization_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    org_name: &str,
) -> Vec<Model> {
    use crate::schema::{gh_organization, gh_repository};
    module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            module::technical_name
                .eq(technical_name)
                .and(gh_organization::name.eq(org_name)),
        )
        .select(Model::as_select())
        .load::<Model>(conn)
        .expect("DB error in module::get_by_technical_name_organization_name")
}

pub fn get_by_technical_name_odoo_version_repository_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
    repo_name: &str,
) -> Vec<Model> {
    use crate::schema::gh_repository;
    module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .filter(
            module::technical_name
                .eq(technical_name)
                .and(module::version_odoo.eq(*version_odoo as i32))
                .and(gh_repository::name.eq(repo_name)),
        )
        .select(Model::as_select())
        .load::<Model>(conn)
        .expect("DB error in module::get_by_technical_name_odoo_version_repository_name")
}

pub fn get_generic_info(
    conn: &mut SqliteConnection,
    technical_name: &str,
) -> Vec<ModuleGenericInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ',') as versions, \
         gh_org.name || '/' || gh_repo.name as src \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON gh_repo.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         WHERE mod.technical_name LIKE ? \
         GROUP BY mod.technical_name, src",
    )
    .bind::<diesel::sql_types::Text, _>(format!("%{technical_name}%"))
    .load::<ModuleGenericInfo>(conn)
    .expect("DB error in module::get_generic_info")
}

pub fn get_generic_info_by_odoo_version(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
) -> Vec<ModuleGenericInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ',') as versions, \
         gh_org.name || '/' || gh_repo.name as src \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON gh_repo.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         WHERE mod.technical_name LIKE ? AND mod.version_odoo = ? \
         GROUP BY mod.technical_name, src",
    )
    .bind::<diesel::sql_types::Text, _>(format!("%{technical_name}%"))
    .bind::<diesel::sql_types::Integer, _>(*version_odoo as i32)
    .load::<ModuleGenericInfo>(conn)
    .expect("DB error in module::get_generic_info_by_odoo_version")
}

pub fn get_generic_info_by_odoo_version_installable(
    conn: &mut SqliteConnection,
    technical_name: &str,
    version_odoo: &u8,
    installable: &bool,
) -> Vec<ModuleGenericInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ',') as versions, \
         gh_org.name || '/' || gh_repo.name as src \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON gh_repo.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         WHERE mod.technical_name LIKE ? AND mod.version_odoo = ? AND mod.installable = ? \
         GROUP BY mod.technical_name, src",
    )
    .bind::<diesel::sql_types::Text, _>(format!("%{technical_name}%"))
    .bind::<diesel::sql_types::Integer, _>(*version_odoo as i32)
    .bind::<diesel::sql_types::Bool, _>(*installable)
    .load::<ModuleGenericInfo>(conn)
    .expect("DB error in module::get_generic_info_by_odoo_version_installable")
}

pub fn get_generic_info_by_installable(
    conn: &mut SqliteConnection,
    technical_name: &str,
    installable: &bool,
) -> Vec<ModuleGenericInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, GROUP_CONCAT(mod.version_odoo, ',') as versions, \
         gh_org.name || '/' || gh_repo.name as src \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON gh_repo.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         WHERE mod.technical_name LIKE ? AND mod.installable = ? \
         GROUP BY mod.technical_name, src",
    )
    .bind::<diesel::sql_types::Text, _>(format!("%{technical_name}%"))
    .bind::<diesel::sql_types::Bool, _>(*installable)
    .load::<ModuleGenericInfo>(conn)
    .expect("DB error in module::get_generic_info_by_installable")
}

/// Cross-repository discovery by category / free-text term / reverse Odoo
/// dependency (which modules depend on `depends_on`), unlike search_modules
/// (technical_name only) or list_repository_modules (needs a known
/// repository first). All filters are optional and combined with the
/// `(? IS NULL OR ...)` idiom so this stays one query instead of the
/// combinatorial set of hand-written variants used by get_generic_info*.
#[allow(clippy::too_many_arguments)]
pub fn search_by_criteria(
    conn: &mut SqliteConnection,
    version_odoo: &u8,
    search_term: Option<&str>,
    category: Option<&str>,
    depends_on: Option<&str>,
    limit: i64,
) -> Vec<ModuleCriteriaInfo> {
    let search_pattern = search_term.map(|s| format!("%{s}%"));
    diesel::sql_query(
        "SELECT mod.technical_name, mod.name, mod.category, mod.installable, mod.application, \
         gh_org.name as organization, gh_repo.name as repository \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON gh_repo.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         WHERE mod.version_odoo = ? \
           AND (? IS NULL OR mod.technical_name LIKE ? OR mod.name LIKE ? OR mod.description LIKE ?) \
           AND (? IS NULL OR mod.category = ?) \
           AND (? IS NULL OR mod.id IN ( \
                 SELECT dep_mod.module_id FROM dependency_module as dep_mod \
                 INNER JOIN dependency as dep ON dep.id = dep_mod.dependency_id \
                 INNER JOIN dependency_type as dt ON dt.id = dep.dependency_type_id \
                 WHERE dt.name = 'module' AND dep.name = ? \
               )) \
         ORDER BY mod.technical_name \
         LIMIT ?",
    )
    .bind::<diesel::sql_types::Integer, _>(*version_odoo as i32)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(search_pattern.clone())
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(search_pattern.clone())
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(search_pattern.clone())
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(search_pattern)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(category)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(category)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(depends_on)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Text>, _>(depends_on)
    .bind::<diesel::sql_types::BigInt, _>(limit)
    .load::<ModuleCriteriaInfo>(conn)
    .expect("DB error in module::search_by_criteria")
}

pub fn get_info(conn: &mut SqliteConnection, technical_name: &str) -> Vec<ModuleInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, mod.name, mod.version_odoo, gh_org.name as organization, \
         gh_rep.name as repository \
         FROM module as mod \
         INNER JOIN gh_repository as gh_rep ON gh_rep.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_rep.gh_organization_id \
         WHERE mod.technical_name = ?",
    )
    .bind::<diesel::sql_types::Text, _>(technical_name)
    .load::<ModuleInfo>(conn)
    .expect("DB error in module::get_info")
}

pub fn count(conn: &mut SqliteConnection) -> Vec<ModuleCountInfo> {
    diesel::sql_query("SELECT version_odoo, count(*) as count FROM module GROUP BY version_odoo")
        .load::<ModuleCountInfo>(conn)
        .expect("DB error in module::count")
}

pub fn count_distinct(conn: &mut SqliteConnection) -> i64 {
    diesel::sql_query("SELECT count(DISTINCT technical_name) as count FROM module")
        .load::<ModuleDistinctCountInfo>(conn)
        .expect("DB error in module::count_distinct")
        .first()
        .map(|x| x.count)
        .unwrap_or(0)
}

pub fn count_organization(conn: &mut SqliteConnection) -> Vec<ModuleCountByOrganizationInfo> {
    diesel::sql_query(
        "SELECT mod.version_odoo, count(*) as count, org.name as org_name \
         FROM module as mod \
         INNER JOIN gh_repository as repo ON mod.gh_repository_id = repo.id \
         INNER JOIN gh_organization as org ON repo.gh_organization_id = org.id \
         GROUP BY org.id, mod.version_odoo \
         ORDER BY count DESC",
    )
    .load::<ModuleCountByOrganizationInfo>(conn)
    .expect("DB error in module::count_organization")
}

pub fn rank_contributor(conn: &mut SqliteConnection) -> Vec<ModuleRankContributorInfo> {
    diesel::sql_query(
        "SELECT * FROM (\
           SELECT mod.version_odoo, count(*) as count, au.name as contrib_name, \
                  RANK() OVER (PARTITION BY mod.version_odoo ORDER BY count(*) DESC) AS rank \
           FROM module as mod \
           INNER JOIN module_author as mod_au ON mod.id = mod_au.module_id \
           INNER JOIN author as au ON mod_au.author_id = au.id \
           WHERE au.name NOT LIKE '% (OCA)' AND au.name NOT LIKE 'OpenERP %' \
                 AND au.name NOT LIKE 'Odoo %' \
           GROUP BY au.id, mod.version_odoo \
           ORDER BY count DESC \
         ) WHERE rank <= 5 ORDER BY rank ASC",
    )
    .load::<ModuleRankContributorInfo>(conn)
    .expect("DB error in module::rank_contributor")
}

pub fn rank_committer(conn: &mut SqliteConnection) -> Vec<ModuleRankCommitterInfo> {
    diesel::sql_query(format!(
        "SELECT * FROM (\
           SELECT mod.version_odoo, SUM(mod_com.commits) as count, com.name as committer_name, \
                  RANK() OVER (PARTITION BY mod.version_odoo ORDER BY SUM(mod_com.commits) DESC) AS rank \
           FROM module as mod \
           INNER JOIN module_committer as mod_com ON mod.id = mod_com.module_id \
           INNER JOIN committer as com ON mod_com.committer_id = com.id \
           WHERE com.name NOT IN ({BOT_COMMITTERS}) \
           GROUP BY com.id, mod.version_odoo \
         ) WHERE rank <= 5 ORDER BY rank ASC"
    ))
    .load::<ModuleRankCommitterInfo>(conn)
    .expect("DB error in module::rank_committer")
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleFunFactInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub organization: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub value: i64,
}

/// The single module (one technical_name/version_odoo/repo row) with the
/// most commits recorded across its committers.
pub fn most_changed(conn: &mut SqliteConnection) -> Option<ModuleFunFactInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, gh_org.name as organization, SUM(mod_com.commits) as value \
         FROM module_committer as mod_com \
         INNER JOIN module as mod ON mod_com.module_id = mod.id \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         GROUP BY mod.id \
         ORDER BY value DESC LIMIT 1",
    )
    .load::<ModuleFunFactInfo>(conn)
    .expect("DB error in module::most_changed")
    .into_iter()
    .next()
}

/// The single module with the most distinct committers.
pub fn most_contributors(conn: &mut SqliteConnection) -> Option<ModuleFunFactInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, gh_org.name as organization, \
         COUNT(DISTINCT mod_com.committer_id) as value \
         FROM module_committer as mod_com \
         INNER JOIN module as mod ON mod_com.module_id = mod.id \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         GROUP BY mod.id \
         ORDER BY value DESC LIMIT 1",
    )
    .load::<ModuleFunFactInfo>(conn)
    .expect("DB error in module::most_contributors")
    .into_iter()
    .next()
}

/// The technical_name present across the most distinct Odoo versions.
pub fn broadest_reach(conn: &mut SqliteConnection) -> Option<ModuleFunFactInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, MIN(gh_org.name) as organization, \
         COUNT(DISTINCT mod.version_odoo) as value \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         GROUP BY mod.technical_name \
         ORDER BY value DESC LIMIT 1",
    )
    .load::<ModuleFunFactInfo>(conn)
    .expect("DB error in module::broadest_reach")
    .into_iter()
    .next()
}

/// The Odoo module depended on by the most other modules (any version).
pub fn most_relied_upon(conn: &mut SqliteConnection) -> Option<ModuleFunFactInfo> {
    diesel::sql_query(
        "SELECT dep.name as technical_name, MIN(gh_org.name) as organization, \
         COUNT(DISTINCT dep_mod.module_id) as value \
         FROM dependency as dep \
         INNER JOIN dependency_type as dep_type ON dep_type.id = dep.dependency_type_id \
         INNER JOIN dependency_module as dep_mod ON dep_mod.dependency_id = dep.id \
         INNER JOIN module as mod ON mod.technical_name = dep.name \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         WHERE dep_type.name = 'module' \
         GROUP BY dep.name \
         ORDER BY value DESC LIMIT 1",
    )
    .load::<ModuleFunFactInfo>(conn)
    .expect("DB error in module::most_relied_upon")
    .into_iter()
    .next()
}

pub fn get_latest_modules_created(conn: &mut SqliteConnection) -> Vec<ModuleLastCreatedInfo> {
    diesel::sql_query(
        "SELECT mod.id, mod.version_odoo, mod.technical_name, date(mod.create_date) as create_date, \
         gh_org.name as org_name \
         FROM module as mod \
         INNER JOIN gh_repository AS gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         ORDER BY mod.create_date DESC LIMIT 10",
    )
    .load::<ModuleLastCreatedInfo>(conn)
    .expect("DB error in module::get_latest_modules_created")
}

pub fn list(conn: &mut SqliteConnection) -> Vec<ModuleListInfo> {
    diesel::sql_query(
        "SELECT DISTINCT mod.technical_name, gh_org.name as org_name, \
         GROUP_CONCAT(mod.version_odoo) as versions_str \
         FROM module as mod \
         INNER JOIN gh_repository AS gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         GROUP BY gh_org.name, mod.technical_name \
         ORDER BY mod.technical_name, gh_org.name",
    )
    .load::<ModuleListRow>(conn)
    .expect("DB error in module::list")
    .into_iter()
    .map(|row| ModuleListInfo {
        technical_name: row.technical_name,
        org_name: row.org_name,
        versions_odoo: row
            .versions_str
            .split(',')
            .filter_map(|s| s.trim().parse::<i32>().ok())
            .collect(),
    })
    .collect()
}

pub fn get_odoo_versions(conn: &mut SqliteConnection) -> Vec<i32> {
    module::table
        .select(module::version_odoo)
        .distinct()
        .order(module::version_odoo.desc())
        .load::<i32>(conn)
        .expect("DB error in module::get_odoo_versions")
}

pub fn get_module_repository(
    conn: &mut SqliteConnection,
    version_odoo: &u8,
    modules: &[String],
) -> Vec<ModuleRepositoryInfo> {
    use crate::schema::gh_repository;
    if modules.is_empty() {
        return vec![];
    }
    let rows = module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .filter(
            module::technical_name
                .eq_any(modules)
                .and(module::version_odoo.eq(*version_odoo as i32)),
        )
        .select((module::technical_name, gh_repository::name))
        .load::<(String, String)>(conn)
        .expect("DB error in module::get_module_repository");

    // One row per technical_name (same as the previous GROUP BY behavior).
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|(technical_name, _)| seen.insert(technical_name.clone()))
        .map(|(technical_name, repository_name)| ModuleRepositoryInfo {
            technical_name,
            repository_name,
        })
        .collect()
}

/// Deletes modules that vanished from a repo since the previous run. FK
/// enforcement is off (see lib.rs), so nothing cascades automatically - the
/// module_version rows for these modules (and their module_view/module_model/
/// module_record snapshots) are deleted by hand first, otherwise they'd be
/// left orphaned forever instead of just for one run's worth of stale data.
pub fn delete_outdated(
    conn: &mut SqliteConnection,
    gh_repo_id: &i64,
    version_odoo: &u8,
    module_ids: &[i64],
) -> QueryResult<usize> {
    if module_ids.is_empty() {
        return Ok(0);
    }
    let stale_ids: Vec<i64> = module::table
        .filter(
            module::gh_repository_id
                .eq(gh_repo_id)
                .and(module::version_odoo.eq(*version_odoo as i32))
                .and(module::id.ne_all(module_ids)),
        )
        .select(module::id)
        .load(conn)?;

    for stale_id in &stale_ids {
        module_model::delete_by_module_id(conn, stale_id)?;
        module_view::delete_by_module_id(conn, stale_id)?;
        module_record::delete_by_module_id(conn, stale_id)?;
        module_version::delete_by_module_id(conn, stale_id)?;
    }

    diesel::delete(module::table.filter(module::id.eq_any(&stale_ids))).execute(conn)
}

pub fn add(conn: &mut SqliteConnection, module_info: &ManifestInfo) -> QueryResult<Model> {
    let gh_org = gh_organization::add(conn, module_info.git_org.as_str())?;
    let gh_repo = gh_repository::add(conn, &gh_org.id, module_info.git_repo.as_str())?;

    let description = if module_info.description.is_empty() {
        None
    } else {
        Some(module_info.description.as_str())
    };
    let installation = if module_info.installation.is_empty() {
        None
    } else {
        Some(module_info.installation.as_str())
    };
    let usage = if module_info.usage.is_empty() {
        None
    } else {
        Some(module_info.usage.as_str())
    };
    let website = if module_info.website.is_empty() {
        None
    } else {
        Some(module_info.website.as_str())
    };
    let license = if module_info.license.is_empty() {
        None
    } else {
        Some(module_info.license.as_str())
    };
    let category = if module_info.category.is_empty() {
        None
    } else {
        Some(module_info.category.as_str())
    };
    let last_commit_partof = if module_info.last_commit_partof.is_empty() {
        None
    } else {
        Some(module_info.last_commit_partof.as_str())
    };

    let existing = get_by_technical_name(
        conn,
        module_info.technical_name.as_str(),
        &module_info.version_odoo,
        &gh_repo.id,
    );

    if existing.is_none() {
        let create_date = get_sqlite_utc_now();
        diesel::insert_into(module::table)
            .values(NewModule {
                technical_name: &module_info.technical_name,
                version_odoo: module_info.version_odoo as i32,
                name: &module_info.name,
                version_module: &module_info.version_module,
                description,
                installation,
                usage,
                website,
                license,
                category,
                auto_install: module_info.auto_install,
                application: module_info.application,
                installable: module_info.installable,
                gh_repository_id: gh_repo.id,
                create_date: &create_date,
                update_date: &create_date,
                folder_size: module_info.folder_size as i64,
                last_commit_hash: &module_info.last_commit_hash,
                last_commit_author: &module_info.last_commit_author,
                last_commit_name: &module_info.last_commit_name,
                last_commit_date: &module_info.last_commit_date,
                last_commit_partof,
            })
            .execute(conn)?;
        let new_id = crate::models::last_insert_rowid(conn);
        let new_module = Model {
            id: new_id,
            technical_name: module_info.technical_name.clone(),
            version_odoo: module_info.version_odoo as i32,
            name: module_info.name.clone(),
            version_module: module_info.version_module.clone(),
            description: description.map(|s| s.to_string()),
            installation: installation.map(|s| s.to_string()),
            usage: usage.map(|s| s.to_string()),
            website: website.map(|s| s.to_string()),
            license: license.map(|s| s.to_string()),
            category: category.map(|s| s.to_string()),
            auto_install: module_info.auto_install,
            application: module_info.application,
            installable: module_info.installable,
            gh_repository_id: gh_repo.id,
            create_date: create_date.clone(),
            update_date: create_date,
            folder_size: module_info.folder_size as i64,
            last_commit_hash: module_info.last_commit_hash.clone(),
            last_commit_author: module_info.last_commit_author.clone(),
            last_commit_name: module_info.last_commit_name.clone(),
            last_commit_date: module_info.last_commit_date.clone(),
            last_commit_partof: last_commit_partof.map(|s| s.to_string()),
        };

        for item in module_info
            .author
            .split(',')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
        {
            module_author::add(conn, &new_module.id, item)?;
        }
        for item in module_info
            .maintainer
            .split(',')
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
        {
            module_maintainer::add(conn, &new_module.id, item)?;
        }
        for (com_name, activity) in &module_info.committers {
            let mc = module_committer::add(conn, &new_module.id, com_name.as_str(), activity)?;
            module_committer_period::replace_for_committer(
                conn,
                &new_module.id,
                &mc.committer_id,
                &activity.periods,
            )?;
        }

        let _ = system_event::register_new_module(
            conn,
            module_info.technical_name.as_str(),
            module_info.name.as_str(),
            module_info.version_module.as_str(),
            module_info.git_org.as_str(),
            module_info.git_repo.as_str(),
            odoo_version_u8_to_string(&module_info.version_odoo).as_str(),
        );
        return Ok(new_module);
    }

    let existing_module = existing.unwrap();

    // Update committers
    for (com_name, activity) in &module_info.committers {
        let mc = module_committer::add(conn, &existing_module.id, com_name.as_str(), activity)?;
        module_committer_period::replace_for_committer(
            conn,
            &existing_module.id,
            &mc.committer_id,
            &activity.periods,
        )?;
    }

    // Check authors
    let authors: Vec<String> = module_info
        .author
        .split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    let module_authors = module_author::get_names_by_module_id(conn, &existing_module.id);
    for author_name in module_authors
        .iter()
        .filter(|item| !authors.contains(item))
        .collect::<Vec<_>>()
    {
        if let Some(a) = author::get_by_name(conn, author_name) {
            module_author::delete_by_module_id_author_id(conn, &existing_module.id, &a.id)?;
            let _ = system_event::register_delete_module_author(
                conn,
                &a.name,
                &existing_module.technical_name,
                &existing_module.name,
                odoo_version_u8_to_string(&(existing_module.version_odoo as u8)).as_str(),
            );
        }
    }
    for author_name in authors
        .iter()
        .filter(|item| !module_authors.contains(&item.to_string()))
        .collect::<Vec<_>>()
    {
        module_author::add(conn, &existing_module.id, author_name)?;
    }

    // Check maintainers
    let maintainers: Vec<String> = module_info
        .maintainer
        .split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect();
    let module_maintainers = module_maintainer::get_names_by_module_id(conn, &existing_module.id);
    for maint_name in module_maintainers
        .iter()
        .filter(|item| !maintainers.contains(item))
        .collect::<Vec<_>>()
    {
        if let Some(m) = maintainer::get_by_name(conn, maint_name) {
            module_maintainer::delete_by_module_id_maintainer_id(conn, &existing_module.id, &m.id)?;
            let _ = system_event::register_delete_module_maintainer(
                conn,
                &m.name,
                &existing_module.technical_name,
                &existing_module.name,
                odoo_version_u8_to_string(&(existing_module.version_odoo as u8)).as_str(),
            );
        }
    }
    for maint_name in maintainers
        .iter()
        .filter(|item| !module_maintainers.contains(&item.to_string()))
        .collect::<Vec<_>>()
    {
        module_maintainer::add(conn, &existing_module.id, maint_name)?;
    }

    // Check for field changes
    let mut changes: Vec<(&str, &str, &str)> = Vec::new();
    let auto_install_str = module_info.auto_install.to_string();
    let installable_str = module_info.installable.to_string();
    let application_str = module_info.application.to_string();
    let folder_size_str = (module_info.folder_size as i64).to_string();
    let module_auto_install_str = existing_module.auto_install.to_string();
    let module_installable_str = existing_module.installable.to_string();
    let module_application_str = existing_module.application.to_string();
    let module_folder_size_str = existing_module.folder_size.to_string();

    if existing_module.name != module_info.name {
        changes.push(("Name", &existing_module.name, &module_info.name));
    }
    if existing_module.version_module != module_info.version_module {
        changes.push((
            "Version Module",
            &existing_module.version_module,
            &module_info.version_module,
        ));
    }
    let existing_desc = existing_module.description_str().to_string();
    if existing_desc != module_info.description {
        changes.push(("Description", &existing_desc, &module_info.description));
    }
    let existing_installation = existing_module.installation_str().to_string();
    if existing_installation != module_info.installation {
        changes.push((
            "Installation",
            &existing_installation,
            &module_info.installation,
        ));
    }
    let existing_usage = existing_module.usage_str().to_string();
    if existing_usage != module_info.usage {
        changes.push(("Usage", &existing_usage, &module_info.usage));
    }
    let existing_website = existing_module.website_str().to_string();
    if existing_website != module_info.website {
        changes.push(("Website", &existing_website, &module_info.website));
    }
    let existing_license = existing_module.license_str().to_string();
    if existing_license != module_info.license {
        changes.push(("License", &existing_license, &module_info.license));
    }
    let existing_category = existing_module.category_str().to_string();
    if existing_category != module_info.category {
        changes.push(("Category", &existing_category, &module_info.category));
    }
    if existing_module.auto_install != module_info.auto_install {
        changes.push(("Auto Install", &module_auto_install_str, &auto_install_str));
    }
    if existing_module.installable != module_info.installable {
        changes.push(("Installable", &module_installable_str, &installable_str));
    }
    if existing_module.application != module_info.application {
        changes.push(("Application", &module_application_str, &application_str));
    }
    if existing_module.folder_size != module_info.folder_size as i64 {
        changes.push(("Folder Size", &module_folder_size_str, &folder_size_str));
    }

    if changes.is_empty() {
        return Ok(existing_module);
    }

    let update_date = get_sqlite_utc_now();
    diesel::update(module::table.filter(module::id.eq(existing_module.id)))
        .set((
            module::name.eq(&module_info.name),
            module::version_module.eq(&module_info.version_module),
            module::description.eq(description),
            module::installation.eq(installation),
            module::usage.eq(usage),
            module::website.eq(website),
            module::license.eq(license),
            module::category.eq(category),
            module::auto_install.eq(module_info.auto_install),
            module::application.eq(module_info.application),
            module::installable.eq(module_info.installable),
            module::update_date.eq(&update_date),
            module::folder_size.eq(module_info.folder_size as i64),
        ))
        .execute(conn)?;

    let odoo_ver = odoo_version_u8_to_string(&module_info.version_odoo);
    let log_info = LogUpdateModuleInfo {
        module_technical_name: module_info.technical_name.as_str(),
        module_name: module_info.name.as_str(),
        module_version: module_info.version_module.as_str(),
        org_name: module_info.git_org.as_str(),
        repo_name: module_info.git_repo.as_str(),
        module_version_odoo: odoo_ver.as_str(),
        module_changes: &changes,
        last_commit_hash: &existing_module.last_commit_hash,
        last_commit_author: &existing_module.last_commit_author,
        last_commit_date: &existing_module.last_commit_date,
        last_commit_name: &existing_module.last_commit_name,
        last_commit_partof: existing_module.last_commit_partof_str(),
    };
    let _ = system_event::register_update_module(conn, &log_info);

    get_by_technical_name(
        conn,
        module_info.technical_name.as_str(),
        &module_info.version_odoo,
        &gh_repo.id,
    )
    .ok_or_else(|| diesel::result::Error::NotFound)
}
