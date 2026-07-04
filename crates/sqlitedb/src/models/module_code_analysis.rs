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

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ModuleAnalysisInfo {
    pub views: Vec<ViewAnalysisInfo>,
    pub models: Vec<ModelAnalysisInfo>,
}
