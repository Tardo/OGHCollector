// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::{module_model, module_model_field, module_model_method};

use super::module_code_analysis::ModelAnalysisInfo;
use super::{module_model_field as field_ops, module_model_method as method_ops};

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_model, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub model_name: String,
    pub class_name: String,
    pub inherit_from: Option<String>,
    pub is_new_model: bool,
    pub docstring: Option<String>,
    pub attrs: Option<String>,
    pub module_version_id: i64,
}

impl Model {
    /// Parses the `attrs` JSON-text column back into a JSON value for API
    /// responses. `None` if there were no attrs, or the stored text somehow
    /// isn't valid JSON (should not happen - we only ever write what we read
    /// back from serde_json ourselves).
    pub fn attrs_value(&self) -> Option<serde_json::Value> {
        self.attrs
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
    }
}

#[derive(Insertable)]
#[diesel(table_name = module_model)]
struct NewModuleModel<'a> {
    module_id: i64,
    model_name: &'a str,
    class_name: &'a str,
    inherit_from: Option<&'a str>,
    is_new_model: bool,
    docstring: Option<&'a str>,
    attrs: Option<&'a str>,
    module_version_id: i64,
}

/// All models ever recorded for this module, across every historical version
/// (uses the denormalized `module_id` column, no join through module_version).
pub fn get_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> Vec<Model> {
    module_model::table
        .filter(module_model::module_id.eq(module_id))
        .order(module_model::model_name.asc())
        .load::<Model>(conn)
        .expect("DB error in module_model::get_by_module_id")
}

/// Models for one specific version snapshot - what callers resolving "latest"
/// or a historical `version_module` actually want.
pub fn get_by_module_version_id(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<Model> {
    module_model::table
        .filter(module_model::module_version_id.eq(module_version_id))
        .order(module_model::model_name.asc())
        .load::<Model>(conn)
        .expect("DB error in module_model::get_by_module_version_id")
}

/// Deletes every model (and their fields/methods) for this module, across all
/// versions - used when the module itself is being removed entirely, not on
/// routine re-analysis (see `replace_for_module`, which is scoped tighter).
pub fn delete_by_module_id(conn: &mut SqliteConnection, module_id: &i64) -> QueryResult<()> {
    let existing_ids: Vec<i64> = module_model::table
        .filter(module_model::module_id.eq(module_id))
        .select(module_model::id)
        .load(conn)?;

    if !existing_ids.is_empty() {
        diesel::delete(
            module_model_field::table
                .filter(module_model_field::module_model_id.eq_any(&existing_ids)),
        )
        .execute(conn)?;
        diesel::delete(
            module_model_method::table
                .filter(module_model_method::module_model_id.eq_any(&existing_ids)),
        )
        .execute(conn)?;
        diesel::delete(module_model::table.filter(module_model::module_id.eq(module_id)))
            .execute(conn)?;
    }

    Ok(())
}

/// Replaces every model (and their fields/methods) for this version snapshot
/// with `models`. FK enforcement is off (see lib.rs), so children must be
/// deleted by hand before the parent rows disappear - otherwise re-analyzing
/// a module leaves module_model_field/module_model_method rows pointing at
/// dead ids. Scoped to `module_version_id`, not `module_id`: filtering either
/// the lookup or the deletes by `module_id` here would wipe every historical
/// version's snapshot on every run, defeating the point of `module_version`.
pub fn replace_for_module(
    conn: &mut SqliteConnection,
    module_id: &i64,
    module_version_id: &i64,
    models: &[ModelAnalysisInfo],
) -> QueryResult<()> {
    let existing_ids: Vec<i64> = module_model::table
        .filter(module_model::module_version_id.eq(module_version_id))
        .select(module_model::id)
        .load(conn)?;

    if !existing_ids.is_empty() {
        diesel::delete(
            module_model_field::table
                .filter(module_model_field::module_model_id.eq_any(&existing_ids)),
        )
        .execute(conn)?;
        diesel::delete(
            module_model_method::table
                .filter(module_model_method::module_model_id.eq_any(&existing_ids)),
        )
        .execute(conn)?;
        diesel::delete(
            module_model::table.filter(module_model::module_version_id.eq(module_version_id)),
        )
        .execute(conn)?;
    }

    for model_info in models {
        let inherit_from = if model_info.inherit_from.is_empty() {
            None
        } else {
            Some(model_info.inherit_from.join(","))
        };
        let attrs = model_info.attrs.as_ref().map(|v| v.to_string());
        diesel::insert_into(module_model::table)
            .values(NewModuleModel {
                module_id: *module_id,
                model_name: model_info.model_name.as_str(),
                class_name: model_info.class_name.as_str(),
                inherit_from: inherit_from.as_deref(),
                is_new_model: model_info.is_new_model,
                docstring: model_info.docstring.as_deref(),
                attrs: attrs.as_deref(),
                module_version_id: *module_version_id,
            })
            .execute(conn)?;
        let module_model_id = crate::models::last_insert_rowid(conn);

        field_ops::add_many(conn, &module_model_id, &model_info.fields)?;
        method_ops::add_many(conn, &module_model_id, &model_info.methods)?;
    }

    Ok(())
}
