// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_security_warning;

// Severity values, matching system_event's string convention. "error" is
// grave (surfaced on the module detail page); "warning" only goes to the
// system_event log.
pub const SEVERITY_ERROR: &str = "error";
pub const SEVERITY_WARNING: &str = "warning";

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_security_warning, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub xml_id: Option<String>,
    pub module_version_id: i64,
}

/// One security finding computed by the collector (see
/// collector::security::analyze_records) from a module's analyzed records.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SecurityWarningInfo {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub xml_id: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = module_security_warning)]
struct NewModuleSecurityWarning<'a> {
    module_id: i64,
    severity: &'a str,
    code: &'a str,
    message: &'a str,
    xml_id: Option<&'a str>,
    module_version_id: i64,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleSecurityWarningFullInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub severity: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub code: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub message: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub xml_id: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
}

/// Every warning for every module's *current* snapshot (mirrors
/// module_version::resolve_current, joined in SQL to avoid an N+1 query per
/// module) - for the site-wide modules overview page. Unlike the module
/// detail page, this includes "warning" (minor) severity too, not just
/// "error", since the whole point of this list is "by severity".
pub fn get_all_current(conn: &mut SqliteConnection) -> Vec<ModuleSecurityWarningFullInfo> {
    diesel::sql_query(
        "SELECT msw.severity, msw.code, msw.message, msw.xml_id, \
         mod.version_odoo, mod.technical_name, gh_org.name as org_name \
         FROM module_security_warning as msw \
         INNER JOIN module_version as mv ON mv.id = msw.module_version_id \
         INNER JOIN module as mod ON mod.id = msw.module_id AND mod.version_module = mv.version_module \
         INNER JOIN gh_repository as gh_repo ON gh_repo.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         ORDER BY msw.severity ASC, mod.technical_name ASC",
    )
    .load::<ModuleSecurityWarningFullInfo>(conn)
    .expect("DB error in module_security_warning::get_all_current")
}

/// Warnings for one specific version snapshot - what the module detail page
/// and API resolve to.
pub fn get_by_module_version_id(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<Model> {
    module_security_warning::table
        .filter(module_security_warning::module_version_id.eq(module_version_id))
        .order((
            module_security_warning::severity.asc(), // "error" < "warning"
            module_security_warning::xml_id.asc(),
        ))
        .load::<Model>(conn)
        .expect("DB error in module_security_warning::get_by_module_version_id")
}

/// Replaces every warning row for this version snapshot. The collector
/// recomputes the full list on every run, so delete+insert is simpler than
/// diffing - scoped to `module_version_id`, not `module_id`, so re-analyzing
/// the current version never touches older versions' snapshots (mirrors
/// module_record).
pub fn replace_for_module(
    conn: &mut SqliteConnection,
    module_id: &i64,
    module_version_id: &i64,
    warnings: &[SecurityWarningInfo],
) -> QueryResult<()> {
    diesel::delete(
        module_security_warning::table
            .filter(module_security_warning::module_version_id.eq(module_version_id)),
    )
    .execute(conn)?;

    let new_rows: Vec<NewModuleSecurityWarning> = warnings
        .iter()
        .map(|w| NewModuleSecurityWarning {
            module_id: *module_id,
            severity: w.severity.as_str(),
            code: w.code.as_str(),
            message: w.message.as_str(),
            xml_id: w.xml_id.as_deref(),
            module_version_id: *module_version_id,
        })
        .collect();

    if !new_rows.is_empty() {
        diesel::insert_into(module_security_warning::table)
            .values(&new_rows)
            .execute(conn)?;
    }

    Ok(())
}
