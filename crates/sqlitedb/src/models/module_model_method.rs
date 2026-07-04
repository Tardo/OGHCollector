// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_model_method;

use super::module_code_analysis::MethodAnalysisInfo;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_model_method, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_model_id: i64,
    pub name: String,
    pub decorators: Option<String>,
    pub signature: String,
    pub docstring: Option<String>,
}

impl Model {
    /// `decorators` is stored as a JSON array (not comma-joined text): a
    /// decorator can itself carry commas, e.g. `api.depends('a', 'b')`, which
    /// a naive split(',') would shred.
    pub fn decorators_vec(&self) -> Vec<String> {
        self.decorators
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }
}

#[derive(Insertable)]
#[diesel(table_name = module_model_method)]
struct NewModuleModelMethod<'a> {
    module_model_id: i64,
    name: &'a str,
    decorators: Option<&'a str>,
    signature: &'a str,
    docstring: Option<&'a str>,
}

pub fn get_by_module_model_id(conn: &mut SqliteConnection, module_model_id: &i64) -> Vec<Model> {
    module_model_method::table
        .filter(module_model_method::module_model_id.eq(module_model_id))
        .order(module_model_method::name.asc())
        .load::<Model>(conn)
        .expect("DB error in module_model_method::get_by_module_model_id")
}

pub fn add_many(
    conn: &mut SqliteConnection,
    module_model_id: &i64,
    methods: &[MethodAnalysisInfo],
) -> QueryResult<()> {
    if methods.is_empty() {
        return Ok(());
    }
    let encoded_decorators: Vec<Option<String>> = methods
        .iter()
        .map(|m| {
            if m.decorators.is_empty() {
                None
            } else {
                serde_json::to_string(&m.decorators).ok()
            }
        })
        .collect();
    let new_rows: Vec<NewModuleModelMethod> = methods
        .iter()
        .zip(encoded_decorators.iter())
        .map(|(m, decorators)| NewModuleModelMethod {
            module_model_id: *module_model_id,
            name: m.name.as_str(),
            decorators: decorators.as_deref(),
            signature: m.signature.as_str(),
            docstring: m.docstring.as_deref(),
        })
        .collect();
    diesel::insert_into(module_model_method::table)
        .values(&new_rows)
        .execute(conn)?;
    Ok(())
}
