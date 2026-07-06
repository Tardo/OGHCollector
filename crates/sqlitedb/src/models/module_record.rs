// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_record;

use super::module_code_analysis::RecordAnalysisInfo;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_record, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub xml_id: String,
    pub model: String,
    pub noupdate: bool,
    pub fields: Option<String>,
    pub module_version_id: i64,
}

impl Model {
    /// Parses the `fields` JSON-text column back into a JSON value for API
    /// responses. `None` if there were no fields, or the stored text somehow
    /// isn't valid JSON (should not happen - we only ever write what we read
    /// back from serde_json ourselves).
    pub fn fields_value(&self) -> Option<serde_json::Value> {
        self.fields
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
    }
}

#[derive(Insertable)]
#[diesel(table_name = module_record)]
struct NewModuleRecord<'a> {
    module_id: i64,
    xml_id: &'a str,
    model: &'a str,
    noupdate: bool,
    fields: Option<&'a str>,
    module_version_id: i64,
}

/// All records ever recorded for this module, across every historical
/// version (uses the denormalized `module_id` column, no join through
/// module_version).
pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_record::table
        .filter(module_record::module_id.eq(module_id))
        .order((module_record::model.asc(), module_record::xml_id.asc()))
        .load::<Model>(conn)
        .expect("DB error in module_record::get_by_module_id")
}

/// Records for one specific version snapshot - what callers resolving
/// "latest" or a historical `version_module` actually want.
pub fn get_by_module_version_id(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<Model> {
    module_record::table
        .filter(module_record::module_version_id.eq(module_version_id))
        .order((module_record::model.asc(), module_record::xml_id.asc()))
        .load::<Model>(conn)
        .expect("DB error in module_record::get_by_module_version_id")
}

pub fn delete_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> QueryResult<usize> {
    diesel::delete(module_record::table.filter(module_record::module_id.eq(module_id)))
        .execute(conn)
}

/// Replaces every record row for this version snapshot with `records`. The
/// collector recomputes the full list from the module's XML/CSV files on
/// every run, so delete+insert is simpler than diffing (mirrors
/// module_view) - but scoped to `module_version_id`, not `module_id`, so
/// re-analyzing the current version never touches older versions' snapshots.
pub fn replace_for_module(
    conn: &mut SqliteConnection,
    module_id: &i64,
    module_version_id: &i64,
    records: &[RecordAnalysisInfo],
) -> QueryResult<()> {
    diesel::delete(
        module_record::table.filter(module_record::module_version_id.eq(module_version_id)),
    )
    .execute(conn)?;

    let fields_json: Vec<Option<String>> = records
        .iter()
        .map(|r| r.fields.as_ref().map(|v| v.to_string()))
        .collect();
    let new_rows: Vec<NewModuleRecord> = records
        .iter()
        .zip(fields_json.iter())
        .map(|(r, fields)| NewModuleRecord {
            module_id: *module_id,
            xml_id: r.xml_id.as_str(),
            model: r.model.as_str(),
            noupdate: r.noupdate,
            fields: fields.as_deref(),
            module_version_id: *module_version_id,
        })
        .collect();

    if !new_rows.is_empty() {
        diesel::insert_into(module_record::table)
            .values(&new_rows)
            .execute(conn)?;
    }

    Ok(())
}
