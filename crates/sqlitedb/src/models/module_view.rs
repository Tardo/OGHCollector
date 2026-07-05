// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_view;

use super::module_code_analysis::ViewAnalysisInfo;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_view, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub xml_id: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub inherit_xml_id: Option<String>,
    pub view_type: Option<String>,
    pub module_version_id: i64,
}

#[derive(Insertable)]
#[diesel(table_name = module_view)]
struct NewModuleView<'a> {
    module_id: i64,
    xml_id: &'a str,
    name: Option<&'a str>,
    model: Option<&'a str>,
    inherit_xml_id: Option<&'a str>,
    view_type: Option<&'a str>,
    module_version_id: i64,
}

/// All views ever recorded for this module, across every historical version
/// (uses the denormalized `module_id` column, no join through module_version).
pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_view::table
        .filter(module_view::module_id.eq(module_id))
        .order(module_view::xml_id.asc())
        .load::<Model>(conn)
        .expect("DB error in module_view::get_by_module_id")
}

/// Views for one specific version snapshot - what callers resolving "latest"
/// or a historical `version_module` actually want.
pub fn get_by_module_version_id(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<Model> {
    module_view::table
        .filter(module_view::module_version_id.eq(module_version_id))
        .order(module_view::xml_id.asc())
        .load::<Model>(conn)
        .expect("DB error in module_view::get_by_module_version_id")
}

pub fn delete_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> QueryResult<usize> {
    diesel::delete(module_view::table.filter(module_view::module_id.eq(module_id))).execute(conn)
}

/// Replaces every view row for this version snapshot with `views`. The
/// collector recomputes the full list from the module's XML files on every
/// run, so delete+insert is simpler than diffing (mirrors
/// module_committer_period) - but scoped to `module_version_id`, not
/// `module_id`, so re-analyzing the current version never touches older
/// versions' snapshots.
pub fn replace_for_module(
    conn: &mut SqliteConnection,
    module_id: &i64,
    module_version_id: &i64,
    views: &[ViewAnalysisInfo],
) -> QueryResult<()> {
    diesel::delete(module_view::table.filter(module_view::module_version_id.eq(module_version_id)))
        .execute(conn)?;

    let new_rows: Vec<NewModuleView> = views
        .iter()
        .map(|v| NewModuleView {
            module_id: *module_id,
            xml_id: v.xml_id.as_str(),
            name: v.name.as_deref(),
            model: v.model.as_deref(),
            inherit_xml_id: v.inherit_xml_id.as_deref(),
            view_type: v.view_type.as_deref(),
            module_version_id: *module_version_id,
        })
        .collect();

    if !new_rows.is_empty() {
        diesel::insert_into(module_view::table)
            .values(&new_rows)
            .execute(conn)?;
    }

    Ok(())
}
