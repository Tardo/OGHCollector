// Copyright Alexandre D. Díaz
use serde::{Deserialize, Serialize};

// DTOs produced by the collector's source analyzer (Python ast + XML parsing,
// see collector::analyzer::analyze_module_source) and consumed by
// module_view::replace_for_module / module_model::replace_for_module.
// Field names match the JSON emitted by the embedded Python analysis script.
//
// `attrs` fields are a free-form JSON object rather than individual columns:
// Odoo field/model keyword arguments aren't a fixed enumerable set, and the
// point of this data is to be read by an LLM (eventually via MCP), so keeping
// it as structured-but-open JSON beats hand-picking a handful of columns.

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ViewAnalysisInfo {
    pub xml_id: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub inherit_xml_id: Option<String>,
    #[serde(default)]
    pub view_type: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FieldAnalysisInfo {
    pub name: String,
    pub field_type: String,
    pub relation: Option<String>,
    #[serde(default)]
    pub attrs: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MethodAnalysisInfo {
    pub name: String,
    pub decorators: Vec<String>,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub docstring: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModelAnalysisInfo {
    pub model_name: String,
    pub class_name: String,
    pub inherit_from: Vec<String>,
    pub is_new_model: bool,
    #[serde(default)]
    pub docstring: Option<String>,
    #[serde(default)]
    pub attrs: Option<serde_json::Value>,
    pub fields: Vec<FieldAnalysisInfo>,
    pub methods: Vec<MethodAnalysisInfo>,
}

// Every other record a module touches - security groups (res.groups),
// record rules (ir.rule), cron jobs, access rights (from
// ir.model.access.csv), demo/reference data, etc. `noupdate` is resolved at
// analysis time (inherited from the wrapping <data>/<odoo>, per-record
// overridable; always false for CSV rows). ir.ui.view records are excluded
// here - they're already fully covered by `ModuleAnalysisInfo::views`.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct RecordAnalysisInfo {
    pub xml_id: String,
    pub model: String,
    #[serde(default)]
    pub noupdate: bool,
    #[serde(default)]
    pub fields: Option<serde_json::Value>,
    // Module-folder-relative source path and 1-based line the record was
    // found at (CSV: the row; XML: the element carrying this xml_id) - None
    // when the analyzer's line index couldn't resolve it.
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<i32>,
}

// One HTTP endpoint the module exposes (a method decorated with http.route).
// `auth` is the *resolved* value (Odoo defaults applied: "user", or "public"
// for website routes) - None when the route is a pure override of an
// inherited route, whose auth can't be known statically. `csrf: None` means
// the framework default (enabled); only an explicit literal True/False is
// recorded. `uses_sudo` flags any `.sudo()` call inside the method body;
// `checks_token_access` flags the portal pattern of validating record access
// via `_document_check_access(..., access_token)` before acting.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ControllerAnalysisInfo {
    pub class_name: String,
    pub name: String,
    pub routes: Vec<String>,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub http_type: String,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub csrf: Option<bool>,
    #[serde(default)]
    pub website: bool,
    #[serde(default)]
    pub uses_sudo: bool,
    #[serde(default)]
    pub checks_token_access: bool,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub docstring: Option<String>,
    // Module-folder-relative source path and 1-based line of the decorated
    // method (free from the `ast` node - no line index needed).
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<i32>,
}

// A raw, not-yet-judged fact spotted by the analyzer (an old-API base class,
// hand-rolled SQL, deprecated view/QWeb syntax, ...) - collector::migration
// turns these into severity+message findings, the same split analyzer.rs
// uses for records/controllers vs. collector::security. `context` is
// whatever locates the fact for a human (a class name, a view/template
// xml_id); `detail` is kind-specific extra text (e.g. the literal base
// class, the SQL snippet, the import module).
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct MigrationFactInfo {
    pub kind: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    // Module-folder-relative source path and 1-based line the fact was
    // spotted at - None when there's no single line to point to (e.g. the
    // `openerp_manifest` fact, which is about a file's existence).
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub line: Option<i32>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModuleAnalysisInfo {
    pub views: Vec<ViewAnalysisInfo>,
    pub models: Vec<ModelAnalysisInfo>,
    #[serde(default)]
    pub records: Vec<RecordAnalysisInfo>,
    #[serde(default)]
    pub controllers: Vec<ControllerAnalysisInfo>,
    #[serde(default)]
    pub migration_facts: Vec<MigrationFactInfo>,
}
