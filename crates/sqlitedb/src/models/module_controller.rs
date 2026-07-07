// Copyright Alexandre D. Díaz
use diesel::prelude::*;
use serde::{Deserialize, Serialize};

use crate::schema::module_controller;

use super::module_code_analysis::ControllerAnalysisInfo;

#[derive(Queryable, Selectable, Debug, Deserialize, Serialize, Clone)]
#[diesel(table_name = module_controller, check_for_backend(diesel::sqlite::Sqlite))]
pub struct Model {
    pub id: i64,
    pub module_id: i64,
    pub class_name: String,
    pub name: String,
    pub routes: String,
    pub auth: Option<String>,
    pub http_type: String,
    pub methods: Option<String>,
    pub csrf: Option<bool>,
    pub website: bool,
    pub uses_sudo: bool,
    pub signature: String,
    pub docstring: Option<String>,
    pub module_version_id: i64,
}

impl Model {
    /// `routes`/`methods` are stored as JSON array text (like
    /// module_model_method.decorators); parse them back for API responses.
    pub fn routes_vec(&self) -> Vec<String> {
        serde_json::from_str(&self.routes).unwrap_or_default()
    }

    pub fn methods_vec(&self) -> Vec<String> {
        self.methods
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }
}

#[derive(Insertable)]
#[diesel(table_name = module_controller)]
struct NewModuleController<'a> {
    module_id: i64,
    class_name: &'a str,
    name: &'a str,
    routes: &'a str,
    auth: Option<&'a str>,
    http_type: &'a str,
    methods: Option<&'a str>,
    csrf: Option<bool>,
    website: bool,
    uses_sudo: bool,
    signature: &'a str,
    docstring: Option<&'a str>,
    module_version_id: i64,
}

/// Controllers for one specific version snapshot.
pub fn get_by_module_version_id(
    conn: &mut SqliteConnection,
    module_version_id: &i64,
) -> Vec<Model> {
    module_controller::table
        .filter(module_controller::module_version_id.eq(module_version_id))
        .order((
            module_controller::class_name.asc(),
            module_controller::name.asc(),
        ))
        .load::<Model>(conn)
        .expect("DB error in module_controller::get_by_module_version_id")
}

/// Replaces every controller row for this version snapshot (delete+insert,
/// scoped to `module_version_id` - mirrors module_record).
pub fn replace_for_module(
    conn: &mut SqliteConnection,
    module_id: &i64,
    module_version_id: &i64,
    controllers: &[ControllerAnalysisInfo],
) -> QueryResult<()> {
    diesel::delete(
        module_controller::table.filter(module_controller::module_version_id.eq(module_version_id)),
    )
    .execute(conn)?;

    let routes_json: Vec<String> = controllers
        .iter()
        .map(|c| serde_json::to_string(&c.routes).unwrap_or_else(|_| "[]".to_string()))
        .collect();
    let methods_json: Vec<Option<String>> = controllers
        .iter()
        .map(|c| {
            if c.methods.is_empty() {
                None
            } else {
                serde_json::to_string(&c.methods).ok()
            }
        })
        .collect();
    let new_rows: Vec<NewModuleController> = controllers
        .iter()
        .zip(routes_json.iter().zip(methods_json.iter()))
        .map(|(c, (routes, methods))| NewModuleController {
            module_id: *module_id,
            class_name: c.class_name.as_str(),
            name: c.name.as_str(),
            routes: routes.as_str(),
            auth: c.auth.as_deref(),
            http_type: c.http_type.as_str(),
            methods: methods.as_deref(),
            csrf: c.csrf,
            website: c.website,
            uses_sudo: c.uses_sudo,
            signature: c.signature.as_str(),
            docstring: c.docstring.as_deref(),
            module_version_id: *module_version_id,
        })
        .collect();

    if !new_rows.is_empty() {
        diesel::insert_into(module_controller::table)
            .values(&new_rows)
            .execute(conn)?;
    }

    Ok(())
}
