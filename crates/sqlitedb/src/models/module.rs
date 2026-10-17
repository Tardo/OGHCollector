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
    pub icon: Option<String>,
    pub source_path: String,
    pub analysis_version: i32,
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
    pub fn icon_str(&self) -> &str {
        self.icon.as_deref().unwrap_or("")
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

#[derive(Clone, Default)]
pub struct ManifestInfo {
    pub technical_name: String,
    pub source_path: String,
    pub version_odoo: u8,
    pub name: String,
    pub version_module: String,
    pub description: String,
    pub installation: String,
    pub usage: String,
    pub icon: String,
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
    /// Set by the analyzer when `last_commit_hash` matches what's already
    /// stored for this module: `analysis`/`committers` above were left empty
    /// instead of being recomputed (no new commits means nothing to add), so
    /// callers must skip any step that would replace stored analysis data
    /// with these empty defaults.
    pub source_unchanged: bool,
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
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub repository_url: Option<String>,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct ModuleRepositoryInfo {
    pub technical_name: String,
    pub repository_name: String,
    pub organization: String,
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
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    pub org_name: String,
    pub versions_odoo: Vec<i32>,
}

#[derive(QueryableByName, Debug, Serialize)]
pub struct LocalizationCountry {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub code: String,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
}

#[derive(QueryableByName, Debug, Serialize)]
pub struct LocalizationCountryVersion {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub code: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub count: i64,
}

/// Each module's localization country as `l10n.code` (uppercase), if any: the
/// `XX` of a module named `l10n_XX`/`l10n_XX_*`, else its repository's when
/// that is an `l10n-*`/`l10n_*` one: an `l10n_XX…` repo name, else OCA's
/// `l10n-<country>` repos by name (codes as Odoo's modules spell them: `uk`,
/// not `gb`), else the most common `l10n_XX` prefix among its modules (other
/// orgs' repos). So every addon of a country repo counts, not just the
/// `l10n_XX` ones; an unknown repo with no `l10n_XX` module stays out.
// MATERIALIZED: inlined, the per-repo subquery reruns per module row (2.9s vs 0.03s).
// ponytail: manual list of OCA's 51 l10n repos (2026-10); a new one falls back to the prefix rule.
const L10N_MODULES: &str = "WITH l10n_repo_oca(name, code) AS (VALUES \
       ('l10n-argentina', 'ar'), ('l10n-australia', 'au'), ('l10n-austria', 'at'), \
       ('l10n-belarus', 'by'), ('l10n-belgium', 'be'), ('l10n-brazil', 'br'), \
       ('l10n-bulgaria', 'bg'), ('l10n-cambodia', 'kh'), ('l10n-canada', 'ca'), \
       ('l10n-chile', 'cl'), ('l10n-china', 'cn'), ('l10n-colombia', 'co'), \
       ('l10n-costa-rica', 'cr'), ('l10n-croatia', 'hr'), ('l10n-ecuador', 'ec'), \
       ('l10n-estonia', 'ee'), ('l10n-ethiopia', 'et'), ('l10n-finland', 'fi'), \
       ('l10n-france', 'fr'), ('l10n-germany', 'de'), ('l10n-greece', 'gr'), \
       ('l10n-india', 'in'), ('l10n-indonesia', 'id'), ('l10n-iran', 'ir'), \
       ('l10n-ireland', 'ie'), ('l10n-italy', 'it'), ('l10n-japan', 'jp'), \
       ('l10n-luxemburg', 'lu'), ('l10n-macedonia', 'mk'), ('l10n-mexico', 'mx'), \
       ('l10n-morocco', 'ma'), ('l10n-netherlands', 'nl'), ('l10n-norway', 'no'), \
       ('l10n-paraguay', 'py'), ('l10n-peru', 'pe'), ('l10n-poland', 'pl'), \
       ('l10n-portugal', 'pt'), ('l10n-romania', 'ro'), ('l10n-russia', 'ru'), \
       ('l10n-slovenia', 'si'), ('l10n-spain', 'es'), ('l10n-switzerland', 'ch'), \
       ('l10n-taiwan', 'tw'), ('l10n-thailand', 'th'), ('l10n-turkey', 'tr'), \
       ('l10n-ukraine', 'ua'), ('l10n-united-kingdom', 'uk'), ('l10n-uruguay', 'uy'), \
       ('l10n-usa', 'us'), ('l10n-venezuela', 've'), ('l10n-vietnam', 'vn')), \
     l10n_repo AS MATERIALIZED ( \
       SELECT repo.id, coalesce( \
         CASE WHEN repo.name GLOB 'l10n?[a-z][a-z]*' AND substr(repo.name, 8, 1) IN ('', '-', '_') \
           THEN substr(repo.name, 6, 2) END, \
         (SELECT code FROM l10n_repo_oca WHERE l10n_repo_oca.name = repo.name), \
         (SELECT substr(m.technical_name, 6, 2) FROM module AS m \
          WHERE m.gh_repository_id = repo.id AND m.technical_name GLOB 'l10n_[a-z][a-z]*' \
            AND substr(m.technical_name, 8, 1) IN ('', '_') \
          GROUP BY 1 ORDER BY count(*) DESC, 1 LIMIT 1)) AS code \
       FROM gh_repository AS repo WHERE repo.name GLOB 'l10n[-_]*'), \
     l10n AS ( \
       SELECT mod.technical_name, mod.name, mod.description, mod.category, mod.version_odoo, \
         org.id AS org_id, org.name AS org_name, \
         upper(coalesce(CASE WHEN mod.technical_name GLOB 'l10n_[a-z][a-z]*' \
             AND substr(mod.technical_name, 8, 1) IN ('', '_') \
           THEN substr(mod.technical_name, 6, 2) END, l10n_repo.code)) AS code \
       FROM module AS mod \
       INNER JOIN gh_repository AS repo ON mod.gh_repository_id = repo.id \
       INNER JOIN gh_organization AS org ON repo.gh_organization_id = org.id \
       LEFT JOIN l10n_repo ON l10n_repo.id = repo.id \
       WHERE mod.technical_name GLOB 'l10n_*' OR repo.name GLOB 'l10n[-_]*') ";

pub fn localization_countries_by_version(
    conn: &mut SqliteConnection,
) -> Vec<LocalizationCountryVersion> {
    diesel::sql_query(format!(
        "{L10N_MODULES} SELECT code, version_odoo, \
         count(DISTINCT org_id || '/' || technical_name) AS count FROM l10n \
         WHERE code IS NOT NULL GROUP BY code, version_odoo ORDER BY count DESC, code"
    ))
    .load(conn)
    .expect("DB error in module::localization_countries_by_version")
}

/// Countries as in [`L10N_MODULES`], over every version.
pub fn localization_countries(conn: &mut SqliteConnection) -> Vec<LocalizationCountry> {
    diesel::sql_query(format!(
        "{L10N_MODULES} SELECT code, count(DISTINCT org_id || '/' || technical_name) AS count \
         FROM l10n WHERE code IS NOT NULL GROUP BY code ORDER BY count DESC, code"
    ))
    .load(conn)
    .expect("DB error in module::localization_countries")
}

#[derive(QueryableByName)]
struct ModuleListRow {
    #[diesel(sql_type = diesel::sql_types::Text)]
    technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    name: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    description: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    category: Option<String>,
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
    icon: Option<&'a str>,
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
    source_path: &'a str,
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
         gh_rep.name as repository, gh_rep.url as repository_url \
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
    diesel::sql_query("SELECT version_odoo, count(*) as count FROM module GROUP BY version_odoo ORDER BY version_odoo")
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

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct VersionCoverage {
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Integer>)]
    pub prev_version: Option<i32>,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub modules: i64,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub prev_modules: i64,
    /// Modules of `prev_version` that also exist here.
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub ported: i64,
    /// Modules of `prev_version` not here under their name, but under the
    /// one OpenUpgrade records them renamed or merged into here.
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub renamed: i64,
    /// Modules of `prev_version` not here yet (nor renamed), with an open
    /// migration PR here.
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub in_progress: i64,
    /// Modules here that no repository had at `prev_version`.
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub new_modules: i64,
}

// Installable modules by technical name: a module that exists at an Odoo
// version (any org/repo) counts as ported there. Same name = same module,
// unless OpenUpgrade records a rename/merge (`openupgrade_rename`).
//
// Under a workspace pool the unqualified `module`/`gh_repository`/
// `gh_organization` are that workspace's (TEMP views) and the `main.` ones
// everyone's. Migration figures list the workspace's modules, but what is
// known about them (exists at the other version, has a migration PR, became
// another module) comes from every workspace: a dependency kept in another
// workspace is no less ported or pending. `pull_request*` and `dependency*`
// aren't shadowed.
const INSTALLABLE_NAMES: &str =
    "SELECT DISTINCT technical_name, version_odoo FROM module WHERE installable";
const INSTALLABLE_NAMES_ALL: &str =
    "SELECT DISTINCT technical_name, version_odoo FROM main.module WHERE installable";
const MIGRATION_PR_NAMES: &str = "SELECT DISTINCT pm.module_technical_name AS technical_name, \
     pr.version_odoo FROM pull_request AS pr \
     INNER JOIN pull_request_module AS pm ON pm.pull_request_id = pr.id AND pm.is_new";

/// Per Odoo version (newest first), how many installable modules of the
/// previous indexed version made it here (under their name or renamed),
/// are being migrated (open migration PR) or not. One rename step per
/// pair: the same answer as `/migration` for consecutive versions. Counts
/// the workspace's modules, judged against every workspace (see above).
pub fn version_coverage(conn: &mut SqliteConnection) -> Vec<VersionCoverage> {
    diesel::sql_query(format!(
        "WITH tw AS ({INSTALLABLE_NAMES}), tv AS ({INSTALLABLE_NAMES_ALL}), pr AS ({MIGRATION_PR_NAMES}), \
         vers AS (SELECT version_odoo, LAG(version_odoo) OVER (ORDER BY version_odoo) AS prev_version \
                  FROM (SELECT DISTINCT version_odoo FROM tw)), \
         rn AS (SELECT r.version_odoo, r.old_name FROM openupgrade_rename AS r WHERE EXISTS \
                (SELECT 1 FROM tv WHERE tv.version_odoo = r.version_odoo AND tv.technical_name = r.new_name)), \
         gone AS (SELECT v.version_odoo, p.technical_name FROM vers AS v \
                  INNER JOIN tw AS p ON p.version_odoo = v.prev_version WHERE NOT EXISTS \
                  (SELECT 1 FROM tv AS c WHERE c.version_odoo = v.version_odoo \
                   AND c.technical_name = p.technical_name)), \
         gone_kind AS (SELECT g.version_odoo, \
                  EXISTS (SELECT 1 FROM rn WHERE rn.version_odoo = g.version_odoo \
                          AND rn.old_name = g.technical_name) AS renamed, \
                  EXISTS (SELECT 1 FROM pr WHERE pr.version_odoo = g.version_odoo \
                          AND pr.technical_name = g.technical_name) AS in_pr FROM gone AS g) \
         SELECT v.version_odoo, v.prev_version, \
          (SELECT count(*) FROM tw WHERE tw.version_odoo = v.version_odoo) AS modules, \
          (SELECT count(*) FROM tw WHERE tw.version_odoo = v.prev_version) AS prev_modules, \
          (SELECT count(*) FROM tw AS p WHERE p.version_odoo = v.prev_version AND EXISTS \
            (SELECT 1 FROM tv AS c WHERE c.version_odoo = v.version_odoo \
             AND c.technical_name = p.technical_name)) AS ported, \
          (SELECT count(*) FROM gone_kind AS k WHERE k.version_odoo = v.version_odoo \
           AND k.renamed) AS renamed, \
          (SELECT count(*) FROM gone_kind AS k WHERE k.version_odoo = v.version_odoo \
           AND NOT k.renamed AND k.in_pr) AS in_progress, \
          (SELECT count(*) FROM tw AS c WHERE c.version_odoo = v.version_odoo AND NOT EXISTS \
            (SELECT 1 FROM tv AS p WHERE p.version_odoo = v.prev_version \
             AND p.technical_name = c.technical_name)) AS new_modules \
         FROM vers AS v ORDER BY v.version_odoo DESC"
    ))
    .load::<VersionCoverage>(conn)
    .expect("DB error in module::version_coverage")
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct TransitionModule {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub repository: String,
    /// Whether this workspace has it at `from` (else only at `to`). Where
    /// it exists anywhere: `versions`.
    #[diesel(sql_type = diesel::sql_types::Bool)]
    pub scope_from: bool,
    /// Installable modules depending on it, at `from` (at `to` if the
    /// workspace has it there only), in any workspace.
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub dependents: i64,
    /// Every version it's installable at, comma separated, in no set order.
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub versions: String,
    /// Every version with an open migration PR for it, same format.
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub pr_versions: Option<String>,
}

/// Every installable module the workspace has at `from` or `to`, the most
/// depended-on first, with its life across all versions and workspaces.
/// Same name = same module here: the caller applies `openupgrade_rename`.
pub fn version_transition(
    conn: &mut SqliteConnection,
    from: i32,
    to: i32,
) -> Vec<TransitionModule> {
    // ponytail: a name in several repos at one version shows the first by
    // org/repo name; a per-repo answer would need rows per repo.
    diesel::sql_query(format!(
        "WITH tv AS ({INSTALLABLE_NAMES_ALL}), pr AS ({MIGRATION_PR_NAMES}), \
         src AS (SELECT m.technical_name, m.version_odoo, org.name AS org_name, repo.name AS repository, \
                 ROW_NUMBER() OVER (PARTITION BY m.technical_name, m.version_odoo \
                                    ORDER BY org.name, repo.name) AS rn \
                 FROM module AS m \
                 INNER JOIN gh_repository AS repo ON repo.id = m.gh_repository_id \
                 INNER JOIN gh_organization AS org ON org.id = repo.gh_organization_id \
                 WHERE m.installable AND m.version_odoo IN (?1, ?2)), \
         u AS (SELECT technical_name, MAX(version_odoo = ?1) AS scope_from \
               FROM src GROUP BY technical_name), \
         deps AS (SELECT dep.name, d.version_odoo, count(DISTINCT d.technical_name) AS n \
                  FROM dependency AS dep \
                  INNER JOIN dependency_type AS dt ON dt.id = dep.dependency_type_id AND dt.name = 'module' \
                  INNER JOIN dependency_module AS dm ON dm.dependency_id = dep.id \
                  INNER JOIN main.module AS d ON d.id = dm.module_id \
                  WHERE d.version_odoo IN (?1, ?2) AND d.installable GROUP BY dep.name, d.version_odoo), \
         life AS (SELECT technical_name, group_concat(version_odoo) AS versions FROM tv GROUP BY technical_name), \
         prl AS (SELECT technical_name, group_concat(version_odoo) AS versions FROM pr GROUP BY technical_name) \
         SELECT u.technical_name, s.org_name, s.repository, u.scope_from, \
          coalesce(deps.n, 0) AS dependents, life.versions, prl.versions AS pr_versions \
         FROM u \
         INNER JOIN src AS s ON s.technical_name = u.technical_name AND s.rn = 1 \
           AND s.version_odoo = IIF(u.scope_from, ?1, ?2) \
         LEFT JOIN deps ON deps.name = u.technical_name AND deps.version_odoo = s.version_odoo \
         INNER JOIN life ON life.technical_name = u.technical_name \
         LEFT JOIN prl ON prl.technical_name = u.technical_name \
         ORDER BY dependents DESC, u.technical_name"
    ))
    .bind::<diesel::sql_types::Integer, _>(from)
    .bind::<diesel::sql_types::Integer, _>(to)
    .load::<TransitionModule>(conn)
    .expect("DB error in module::version_transition")
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct NamedModule {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
}

/// Every installable module at `version_odoo` in any workspace, with the
/// first organization (by name) that has it.
pub fn installable_everywhere(conn: &mut SqliteConnection, version_odoo: i32) -> Vec<NamedModule> {
    diesel::sql_query(
        "SELECT m.technical_name, MIN(org.name) AS org_name FROM main.module AS m \
         INNER JOIN main.gh_repository AS repo ON repo.id = m.gh_repository_id \
         INNER JOIN main.gh_organization AS org ON org.id = repo.gh_organization_id \
         WHERE m.installable AND m.version_odoo = ? GROUP BY m.technical_name",
    )
    .bind::<diesel::sql_types::Integer, _>(version_odoo)
    .load::<NamedModule>(conn)
    .expect("DB error in module::installable_everywhere")
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct RepositoryVersionCount {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub repository: String,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    pub modules: i64,
}

/// Installable modules per repository and Odoo version.
pub fn repository_version_counts(conn: &mut SqliteConnection) -> Vec<RepositoryVersionCount> {
    diesel::sql_query(
        "SELECT org.name AS org_name, repo.name AS repository, m.version_odoo, \
         count(DISTINCT m.technical_name) AS modules FROM module AS m \
         INNER JOIN gh_repository AS repo ON repo.id = m.gh_repository_id \
         INNER JOIN gh_organization AS org ON org.id = repo.gh_organization_id \
         WHERE m.installable GROUP BY repo.id, m.version_odoo",
    )
    .load::<RepositoryVersionCount>(conn)
    .expect("DB error in module::repository_version_counts")
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

/// The single module (one technical_name/repo row, at the given Odoo
/// version) with the most commits recorded across its committers.
pub fn most_changed(conn: &mut SqliteConnection, version_odoo: &u8) -> Option<ModuleFunFactInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, gh_org.name as organization, SUM(mod_com.commits) as value \
         FROM module_committer as mod_com \
         INNER JOIN module as mod ON mod_com.module_id = mod.id \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         WHERE mod.version_odoo = ? \
         GROUP BY mod.id \
         ORDER BY value DESC LIMIT 1",
    )
    .bind::<diesel::sql_types::Integer, _>(*version_odoo as i32)
    .load::<ModuleFunFactInfo>(conn)
    .expect("DB error in module::most_changed")
    .into_iter()
    .next()
}

/// The single module with the largest folder size, at the given Odoo version.
pub fn largest_module(conn: &mut SqliteConnection, version_odoo: &u8) -> Option<ModuleFunFactInfo> {
    diesel::sql_query(
        "SELECT mod.technical_name, gh_org.name as organization, mod.folder_size as value \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         WHERE mod.version_odoo = ? \
         ORDER BY mod.folder_size DESC LIMIT 1",
    )
    .bind::<diesel::sql_types::Integer, _>(*version_odoo as i32)
    .load::<ModuleFunFactInfo>(conn)
    .expect("DB error in module::largest_module")
    .into_iter()
    .next()
}

/// The most recently added module, at the given Odoo version.
pub fn newest_module(
    conn: &mut SqliteConnection,
    version_odoo: &u8,
) -> Option<ModuleLastCreatedInfo> {
    diesel::sql_query(
        "SELECT mod.id, mod.version_odoo, mod.technical_name, mod.create_date, \
         gh_org.name as org_name \
         FROM module as mod \
         INNER JOIN gh_repository as gh_repo ON mod.gh_repository_id = gh_repo.id \
         INNER JOIN gh_organization as gh_org ON gh_repo.gh_organization_id = gh_org.id \
         WHERE mod.version_odoo = ? \
         ORDER BY mod.create_date DESC LIMIT 1",
    )
    .bind::<diesel::sql_types::Integer, _>(*version_odoo as i32)
    .load::<ModuleLastCreatedInfo>(conn)
    .expect("DB error in module::newest_module")
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
        "SELECT mod.technical_name, MAX(mod.name) as name, MAX(mod.description) as description, \
         MAX(mod.category) as category, gh_org.name as org_name, \
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
        name: row.name,
        description: row.description,
        category: row.category,
        org_name: row.org_name,
        versions_odoo: row
            .versions_str
            .split(',')
            .filter_map(|s| s.trim().parse::<i32>().ok())
            .collect(),
    })
    .collect()
}

pub fn list_localization(
    conn: &mut SqliteConnection,
    code: &str,
    version: Option<i32>,
) -> Vec<ModuleListInfo> {
    diesel::sql_query(format!(
        "{L10N_MODULES} SELECT technical_name, MAX(name) as name, MAX(description) as description, \
         MAX(category) as category, org_name, GROUP_CONCAT(DISTINCT version_odoo) as versions_str \
         FROM l10n WHERE code = ? AND (? IS NULL OR version_odoo = ?) \
         GROUP BY org_name, technical_name ORDER BY technical_name, org_name"
    ))
    .bind::<diesel::sql_types::Text, _>(code.to_uppercase())
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Integer>, _>(version)
    .bind::<diesel::sql_types::Nullable<diesel::sql_types::Integer>, _>(version)
    .load::<ModuleListRow>(conn)
    .expect("DB error in module::list_localization")
    .into_iter()
    .map(|row| ModuleListInfo {
        technical_name: row.technical_name,
        name: row.name,
        description: row.description,
        category: row.category,
        org_name: row.org_name,
        versions_odoo: row
            .versions_str
            .split(',')
            .filter_map(|s| s.parse().ok())
            .collect(),
    })
    .collect()
}

// Base64 `static/description/icon.png` for a module (any version, newest
// first - icons are practically never version-specific, same as name/
// description in `list` above). Empty string means "no icon file", stored
// that way rather than NULL, so it's filtered out like NULL.
pub fn get_icon(
    conn: &mut SqliteConnection,
    org_name: &str,
    technical_name: &str,
) -> Option<String> {
    use crate::schema::{gh_organization, gh_repository};
    module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            gh_organization::name
                .eq(org_name)
                .and(module::technical_name.eq(technical_name))
                .and(module::icon.is_not_null())
                .and(module::icon.ne("")),
        )
        .order(module::version_odoo.desc())
        .select(module::icon)
        .first::<Option<String>>(conn)
        .ok()
        .flatten()
}

pub fn get_odoo_versions(conn: &mut SqliteConnection) -> Vec<i32> {
    module::table
        .select(module::version_odoo)
        .distinct()
        .order(module::version_odoo.desc())
        .load::<i32>(conn)
        .expect("DB error in module::get_odoo_versions")
}

/// Odoo series (ascending) `technical_name` exists in within `org_name`, in
/// any of its repositories (modules move between repos of the same org).
pub fn get_odoo_versions_by_technical_name_organization_name(
    conn: &mut SqliteConnection,
    technical_name: &str,
    org_name: &str,
) -> Vec<i32> {
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
        .select(module::version_odoo)
        .distinct()
        .order(module::version_odoo.asc())
        .load::<i32>(conn)
        .expect("DB error in module::get_odoo_versions_by_technical_name_organization_name")
}

pub fn get_module_repository(
    conn: &mut SqliteConnection,
    version_odoo: &u8,
    modules: &[String],
) -> Vec<ModuleRepositoryInfo> {
    use crate::schema::{gh_organization, gh_repository};
    if modules.is_empty() {
        return vec![];
    }
    let rows = module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(
            module::technical_name
                .eq_any(modules)
                .and(module::version_odoo.eq(*version_odoo as i32)),
        )
        .select((
            module::technical_name,
            gh_repository::name,
            gh_organization::name,
        ))
        .load::<(String, String, String)>(conn)
        .expect("DB error in module::get_module_repository");

    // One row per technical_name (same as the previous GROUP BY behavior).
    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|(technical_name, _, _)| seen.insert(technical_name.clone()))
        .map(
            |(technical_name, repository_name, organization)| ModuleRepositoryInfo {
                technical_name,
                repository_name,
                organization,
            },
        )
        .collect()
}

/// Version-agnostic: tells the doodba migration plan tool which requested
/// modules the system has EVER seen merged, in any version, so it can tell
/// "registered but not ported to this version yet" apart from modules it has
/// simply never tracked. Prefers the most recent version's repo/org when a
/// technical_name moved between repos across versions.
pub fn get_repository_org_by_technical_names(
    conn: &mut SqliteConnection,
    modules: &[String],
) -> Vec<ModuleRepositoryInfo> {
    use crate::schema::{gh_organization, gh_repository};
    if modules.is_empty() {
        return vec![];
    }
    let rows = module::table
        .inner_join(gh_repository::table.on(gh_repository::id.eq(module::gh_repository_id)))
        .inner_join(
            gh_organization::table.on(gh_organization::id.eq(gh_repository::gh_organization_id)),
        )
        .filter(module::technical_name.eq_any(modules))
        .order(module::version_odoo.desc())
        .select((
            module::technical_name,
            gh_repository::name,
            gh_organization::name,
        ))
        .load::<(String, String, String)>(conn)
        .expect("DB error in module::get_repository_org_by_technical_names");

    let mut seen = std::collections::HashSet::new();
    rows.into_iter()
        .filter(|(technical_name, _, _)| seen.insert(technical_name.clone()))
        .map(
            |(technical_name, repository_name, organization)| ModuleRepositoryInfo {
                technical_name,
                repository_name,
                organization,
            },
        )
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
    let icon = if module_info.icon.is_empty() {
        None
    } else {
        Some(module_info.icon.as_str())
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
                icon,
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
                source_path: &module_info.source_path,
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
            icon: icon.map(|s| s.to_string()),
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
            source_path: module_info.source_path.clone(),
            analysis_version: 0,
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
    // Not the raw base64 - it'd bloat the human-readable log message for no
    // benefit (see system_event::register_update_module).
    if existing_module.icon_str() != module_info.icon {
        changes.push(("Icon", "(previous)", "(updated)"));
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

    // A hash-only change (source touched, but no manifest field moved) must
    // still persist - otherwise last_commit_* freezes at the first insert
    // forever (the `changes` list above only tracks manifest fields), and
    // the collector workflows' `unchanged` (collector::workflow) - which
    // compares against this stored hash - would never see a hash it can
    // trust. But a truly unchanged module must stay untouched: update_date
    // is shown on the module page and served over the API/MCP, and bumping
    // it here for every module on every run would be a visible regression.
    let hash_changed = !module_info.last_commit_hash.is_empty()
        && existing_module.last_commit_hash != module_info.last_commit_hash;
    if changes.is_empty() && !hash_changed {
        if existing_module.source_path == module_info.source_path {
            return Ok(existing_module);
        }
        diesel::update(module::table.filter(module::id.eq(existing_module.id)))
            .set(module::source_path.eq(&module_info.source_path))
            .execute(conn)?;
        return get_by_id(conn, &existing_module.id).ok_or(diesel::result::Error::NotFound);
    }

    // A git failure yields empty last_commit_* on `module_info` (see
    // OGHCollectorAnalyzer::get_git_info) - fall back to what's already
    // stored rather than clobbering good commit metadata with blanks.
    let (commit_hash, commit_author, commit_name, commit_date, commit_partof): (
        &str,
        &str,
        &str,
        &str,
        Option<&str>,
    ) = if module_info.last_commit_hash.is_empty() {
        (
            existing_module.last_commit_hash.as_str(),
            existing_module.last_commit_author.as_str(),
            existing_module.last_commit_name.as_str(),
            existing_module.last_commit_date.as_str(),
            existing_module.last_commit_partof.as_deref(),
        )
    } else {
        (
            module_info.last_commit_hash.as_str(),
            module_info.last_commit_author.as_str(),
            module_info.last_commit_name.as_str(),
            module_info.last_commit_date.as_str(),
            last_commit_partof,
        )
    };

    let update_date = get_sqlite_utc_now();
    diesel::update(module::table.filter(module::id.eq(existing_module.id)))
        .set((
            module::name.eq(&module_info.name),
            module::version_module.eq(&module_info.version_module),
            module::description.eq(description),
            module::installation.eq(installation),
            module::usage.eq(usage),
            module::icon.eq(icon),
            module::website.eq(website),
            module::license.eq(license),
            module::category.eq(category),
            module::auto_install.eq(module_info.auto_install),
            module::application.eq(module_info.application),
            module::installable.eq(module_info.installable),
            module::update_date.eq(&update_date),
            module::folder_size.eq(module_info.folder_size as i64),
            module::last_commit_hash.eq(commit_hash),
            module::last_commit_author.eq(commit_author),
            module::last_commit_name.eq(commit_name),
            module::last_commit_date.eq(commit_date),
            module::last_commit_partof.eq(commit_partof),
            module::source_path.eq(&module_info.source_path),
        ))
        .execute(conn)?;

    if !changes.is_empty() {
        let odoo_ver = odoo_version_u8_to_string(&module_info.version_odoo);
        let log_info = LogUpdateModuleInfo {
            module_technical_name: module_info.technical_name.as_str(),
            module_name: module_info.name.as_str(),
            module_version: module_info.version_module.as_str(),
            org_name: module_info.git_org.as_str(),
            repo_name: module_info.git_repo.as_str(),
            module_version_odoo: odoo_ver.as_str(),
            module_changes: &changes,
            last_commit_hash: commit_hash,
            last_commit_author: commit_author,
            last_commit_date: commit_date,
            last_commit_name: commit_name,
            last_commit_partof: commit_partof.unwrap_or(""),
        };
        let _ = system_event::register_update_module(conn, &log_info);
    }

    get_by_technical_name(
        conn,
        module_info.technical_name.as_str(),
        &module_info.version_odoo,
        &gh_repo.id,
    )
    .ok_or_else(|| diesel::result::Error::NotFound)
}

/// Stamps which version of the collector's analysis rules produced this
/// module's stored analysis (see collector::analyzer::ANALYSIS_VERSION).
pub fn set_analysis_version(
    conn: &mut SqliteConnection,
    module_id: &i64,
    version: i32,
) -> QueryResult<usize> {
    diesel::update(module::table.find(module_id))
        .set(module::analysis_version.eq(version))
        .execute(conn)
}
