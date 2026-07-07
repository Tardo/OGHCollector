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
