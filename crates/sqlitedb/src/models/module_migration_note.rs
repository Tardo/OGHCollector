// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_migration_note;

// "warning" = will likely break, or needs an actual code change before
// migrating. "info" = still works today, but worth a second look (e.g. a
// hand-rolled SQL view whose columns the ORM never checks). Deliberately
// text, not "error"/"warning" like module_security_warning - these aren't
// security findings.
pub const SEVERITY_WARNING: &str = "warning";
pub const SEVERITY_INFO: &str = "info";

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_migration_note, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub severity: String,
    pub code: String,
    pub message: String,
    pub context: Option<String>,
    pub module_version_id: i64,
}

/// One migration consideration computed by the collector (see
/// collector::migration) from a module's analyzed models/fields and raw
/// migration_facts.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MigrationConsiderationInfo {
    pub severity: String,
    pub code: String,
    pub message: String,
    pub context: Option<String>,
}

#[derive(Insertable)]
#[diesel(table_name = module_migration_note)]
struct NewModuleMigrationNote<'a> {
    module_id: i64,
    severity: &'a str,
    code: &'a str,
    message: &'a str,
    context: Option<&'a str>,
    module_version_id: i64,
}

#[derive(QueryableByName, Debug, Deserialize, Serialize, Clone)]
pub struct ModuleMigrationNoteFullInfo {
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub severity: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub code: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub message: String,
    #[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Text>)]
    pub context: Option<String>,
    #[diesel(sql_type = diesel::sql_types::Integer)]
    pub version_odoo: i32,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub technical_name: String,
    #[diesel(sql_type = diesel::sql_types::Text)]
    pub org_name: String,
}

/// Every consideration for every module's *current* snapshot (mirrors
/// module_version::resolve_current, joined in SQL to avoid an N+1 query per
/// module) - for the site-wide modules overview page. Both severities are
/// included, split by the caller like module_security_warning::get_all_current.
pub fn get_all_current(conn: &mut SqliteConnection) -> Vec<ModuleMigrationNoteFullInfo> {
    diesel::sql_query(
        "SELECT mmn.severity, mmn.code, mmn.message, mmn.context, \
         mod.version_odoo, mod.technical_name, gh_org.name as org_name \
         FROM module_migration_note as mmn \
         INNER JOIN module_version as mv ON mv.id = mmn.module_version_id \
         INNER JOIN module as mod ON mod.id = mmn.module_id AND mod.version_module = mv.version_module \
         INNER JOIN gh_repository as gh_repo ON gh_repo.id = mod.gh_repository_id \
         INNER JOIN gh_organization as gh_org ON gh_org.id = gh_repo.gh_organization_id \
         ORDER BY mmn.severity DESC, mod.technical_name ASC",
    )
    .load::<ModuleMigrationNoteFullInfo>(conn)
    .expect("DB error in module_migration_note::get_all_current")
}

/// Considerations for one specific version snapshot - what the module detail
/// page and API resolve to. Ordered `severity DESC`: "warning" > "info"
/// alphabetically, so this - unlike module_security_warning's `.asc()` - puts
/// the higher-priority rows first without needing a CASE expression.
pub fn get_by_module_version_id(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<Model> {
    module_migration_note::table
        .filter(module_migration_note::module_version_id.eq(module_version_id))
        .order((
            module_migration_note::severity.desc(),
            module_migration_note::context.asc(),
        ))
        .load::<Model>(conn)
        .expect("DB error in module_migration_note::get_by_module_version_id")
}

/// Replaces every consideration row for this version snapshot (delete+insert,
/// scoped to `module_version_id` - mirrors module_security_warning).
pub fn replace_for_module(
    conn: &mut SqliteConnection,
    module_id: &i64,
    module_version_id: &i64,
    notes: &[MigrationConsiderationInfo],
) -> QueryResult<()> {
    diesel::delete(
        module_migration_note::table
            .filter(module_migration_note::module_version_id.eq(module_version_id)),
    )
    .execute(conn)?;

    let new_rows: Vec<NewModuleMigrationNote> = notes
        .iter()
        .map(|n| NewModuleMigrationNote {
            module_id: *module_id,
            severity: n.severity.as_str(),
            code: n.code.as_str(),
            message: n.message.as_str(),
            context: n.context.as_deref(),
            module_version_id: *module_version_id,
        })
        .collect();

    if !new_rows.is_empty() {
        diesel::insert_into(module_migration_note::table)
            .values(&new_rows)
            .execute(conn)?;
    }

    Ok(())
}
