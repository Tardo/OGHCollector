// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_model_field;

use super::module_code_analysis::FieldAnalysisInfo;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_model_field, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_model_id: i64,
    pub name: String,
    pub field_type: String,
    pub relation: Option<String>,
    pub attrs: Option<String>,
}

impl Model {
    /// Parses the `attrs` JSON-text column (the field's keyword arguments -
    /// string, help, required, readonly, compute, related, default,
    /// selection, ...) back into a JSON value for API responses.
    pub fn attrs_value(&self) -> Option<serde_json::Value> {
        self.attrs
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
    }
}

#[derive(Insertable)]
#[diesel(table_name = module_model_field)]
struct NewModuleModelField<'a> {
    module_model_id: i64,
    name: &'a str,
    field_type: &'a str,
    relation: Option<&'a str>,
    attrs: Option<&'a str>,
}

pub fn get_by_module_model_id(conn: &mut SqliteConnection, module_model_id: &i64) -> Vec<Model> {
    module_model_field::table
        .filter(module_model_field::module_model_id.eq(module_model_id))
        .order(module_model_field::name.asc())
        .load::<Model>(conn)
        .expect("DB error in module_model_field::get_by_module_model_id")
}

pub fn add_many(
    conn: &mut SqliteConnection,
    module_model_id: &i64,
    fields: &[FieldAnalysisInfo],
) -> QueryResult<()> {
    if fields.is_empty() {
        return Ok(());
    }
    let attrs_json: Vec<Option<String>> = fields
        .iter()
        .map(|f| f.attrs.as_ref().map(|v| v.to_string()))
        .collect();
    let new_rows: Vec<NewModuleModelField> = fields
        .iter()
        .zip(attrs_json.iter())
        .map(|(f, attrs)| NewModuleModelField {
            module_model_id: *module_model_id,
            name: f.name.as_str(),
            field_type: f.field_type.as_str(),
            relation: f.relation.as_deref(),
            attrs: attrs.as_deref(),
        })
        .collect();
    diesel::insert_into(module_model_field::table)
        .values(&new_rows)
        .execute(conn)?;
    Ok(())
}
