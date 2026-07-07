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
}

// One HTTP endpoint the module exposes (a method decorated with http.route).
// `auth` is the *resolved* value (Odoo defaults applied: "user", or "public"
// for website routes) - None when the route is a pure override of an
// inherited route, whose auth can't be known statically. `csrf: None` means
// the framework default (enabled); only an explicit literal True/False is
// recorded. `uses_sudo` flags any `.sudo()` call inside the method body.
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
    pub signature: String,
    #[serde(default)]
    pub docstring: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModuleAnalysisInfo {
    pub views: Vec<ViewAnalysisInfo>,
    pub models: Vec<ModelAnalysisInfo>,
    #[serde(default)]
    pub records: Vec<RecordAnalysisInfo>,
    #[serde(default)]
    pub controllers: Vec<ControllerAnalysisInfo>,
}
