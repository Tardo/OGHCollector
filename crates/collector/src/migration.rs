// Copyright Alexandre D. Díaz
//! Static "what to check before/after upgrading this module to a newer Odoo
//! version" findings, mirroring security.rs's shape (severity/code/message)
//! but scoped to compatibility/maintainability rather than security. Facts
//! are extracted by analyzer.rs's embedded Python (kept a pure extractor, no
//! judgement); this module turns them into findings, same split security.rs
//! uses for the records/controllers it judges.
//!
//! Two severities: "warning" (will likely break, or needs an actual code
//! change) and "info" (still works today, but worth a second look).
use sqlitedb::models::module_code_analysis::{MigrationFactInfo, ModelAnalysisInfo};
use sqlitedb::models::module_migration_note::{
    MigrationConsiderationInfo, SEVERITY_INFO, SEVERITY_WARNING,
};

fn note(
    severity: &str,
    code: &str,
    context: Option<String>,
    message: String,
) -> MigrationConsiderationInfo {
    MigrationConsiderationInfo {
        severity: severity.to_string(),
        code: code.to_string(),
        message,
        context,
    }
}

fn attrs_obj(
    attrs: &Option<serde_json::Value>,
) -> Option<&serde_json::Map<String, serde_json::Value>> {
    attrs.as_ref()?.as_object()
}

fn attrs_bool(attrs: &Option<serde_json::Value>, key: &str) -> Option<bool> {
    attrs_obj(attrs)?.get(key)?.as_bool()
}

/// `init_sql` is stored as a JSON array of the literal SQL text found inside
/// a model's `init()` (see analyzer.rs's `_cr_execute_sql`).
fn attrs_str_list(attrs: &Option<serde_json::Value>, key: &str) -> Vec<String> {
    attrs_obj(attrs)
        .and_then(|o| o.get(key))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Findings from a module's analyzed models/fields: `_auto = False` models
/// whose `init()` hand-rolls SQL (the table-vs-view distinction the user
/// asked for), and deprecated field kwargs already captured in `fields[].attrs`.
pub fn analyze_models(models: &[ModelAnalysisInfo]) -> Vec<MigrationConsiderationInfo> {
    let mut out = Vec::new();
    for m in models {
        if attrs_bool(&m.attrs, "auto") == Some(false) {
            for sql in attrs_str_list(&m.attrs, "init_sql") {
                let upper = sql.to_uppercase();
                if upper.contains("CREATE TABLE") {
                    out.push(note(
                        SEVERITY_WARNING,
                        "migration-sql-table",
                        Some(m.model_name.clone()),
                        format!(
                            "Model '{}' has `_auto = False` and its init() creates a real TABLE via raw SQL{}: Odoo's ORM does not manage this table's schema, so column/index changes across versions must be applied by hand (an `IF NOT EXISTS` guard also means an already-existing table silently keeps its old schema after upgrading).",
                            m.model_name,
                            if upper.contains("IF NOT EXISTS") { " (`CREATE TABLE IF NOT EXISTS`)" } else { "" }
                        ),
                    ));
                } else if upper.contains("VIEW") {
                    out.push(note(
                        SEVERITY_INFO,
                        "migration-sql-view",
                        Some(m.model_name.clone()),
                        format!(
                            "Model '{}' has `_auto = False` and its init() (re)creates a SQL VIEW: the standard reporting-model pattern, but its raw SQL isn't checked by the ORM - verify it against renamed/removed tables and columns in the target version.",
                            m.model_name
                        ),
                    ));
                }
            }
        }
        for f in &m.fields {
            if attrs_obj(&f.attrs).is_some_and(|o| o.contains_key("track_visibility")) {
                out.push(note(
                    SEVERITY_WARNING,
                    "migration-track-visibility",
                    Some(format!("{}.{}", m.model_name, f.name)),
                    format!(
                        "Field '{}' on '{}' uses the deprecated `track_visibility` kwarg: renamed to `tracking` in Odoo 13.0.",
                        f.name, m.model_name
                    ),
                ));
            }
        }
    }
    out
}

/// Findings from the raw migration_facts list (old-API classes, hand-rolled
/// SQL writes, deprecated view/QWeb syntax, ...).
pub fn analyze_facts(facts: &[MigrationFactInfo]) -> Vec<MigrationConsiderationInfo> {
    let mut out = Vec::new();
    for f in facts {
        let ctx = f.context.clone();
        let ctx_str = || f.context.as_deref().unwrap_or("?").to_string();
        match f.kind.as_str() {
            "old_api_base" => out.push(note(
                SEVERITY_WARNING,
                "migration-old-api-base",
                ctx,
                format!(
                    "Class '{}' still inherits from the old-style `{}` API: rewrite it against `models.Model`/`models.TransientModel`/`models.AbstractModel` - the old `osv`/`orm` API was removed entirely in modern Odoo.",
                    ctx_str(),
                    f.detail.as_deref().unwrap_or("osv.*")
                ),
            )),
            "old_style_field_dict" => out.push(note(
                SEVERITY_WARNING,
                "migration-old-style-fields",
                ctx,
                format!(
                    "Class '{}' declares `{}` as a plain dict: old-style field/default declarations, replace with `fields.X(...)` class attributes and `default=`.",
                    ctx_str(),
                    f.detail.as_deref().unwrap_or("_columns")
                ),
            )),
            "openerp_import" => out.push(note(
                SEVERITY_WARNING,
                "migration-openerp-import",
                None,
                format!(
                    "Imports from the old `{}` namespace: renamed to `odoo` since Odoo 10.0.",
                    f.detail.as_deref().unwrap_or("openerp")
                ),
            )),
            "workflow_call" => out.push(note(
                SEVERITY_WARNING,
                "migration-workflow",
                None,
                "Uses the old workflow engine (base.workflow / `trg_validate`): removed in Odoo 11.0, must be rewritten as state-field transitions.".to_string(),
            )),
            "raw_sql_write" => out.push(note(
                SEVERITY_INFO,
                "migration-raw-sql-write",
                None,
                format!(
                    "Raw `cr.execute()` INSERT/UPDATE/DELETE bypasses the ORM (no compute/constrains/tracking/mail): `{}` - re-check the table/column names still match after upgrading.",
                    f.detail.as_deref().unwrap_or("")
                ),
            )),
            "view_tree_tag" => out.push(note(
                SEVERITY_INFO,
                "migration-view-tree-tag",
                ctx,
                format!(
                    "View '{}' is defined with a `<tree>` root tag: renamed to `<list>` in Odoo 18.0.",
                    ctx_str()
                ),
            )),
            "view_attrs_states" => out.push(note(
                SEVERITY_WARNING,
                "migration-view-attrs-states",
                ctx,
                format!(
                    "View '{}' uses the deprecated `attrs=`/`states=` syntax: removed in Odoo 17.0+, replace with direct `invisible`/`readonly`/`required` domain expressions.",
                    ctx_str()
                ),
            )),
            "view_t_raw" => out.push(note(
                SEVERITY_WARNING,
                "migration-template-t-raw",
                ctx,
                format!(
                    "Template '{}' uses `t-raw`: removed in Odoo 17.0, replace with `t-out` (auto-escaped) or `Markup`.",
                    ctx_str()
                ),
            )),
            "openerp_manifest" => out.push(note(
                SEVERITY_WARNING,
                "migration-openerp-manifest",
                None,
                "Manifest file is `__openerp__.py`: renamed to `__manifest__.py` since Odoo 10.0 - a strong signal this module hasn't been touched since.".to_string(),
            )),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlitedb::models::module_code_analysis::FieldAnalysisInfo;

    fn model(auto: Option<bool>, init_sql: &[&str]) -> ModelAnalysisInfo {
        let mut attrs = serde_json::json!({});
        if let Some(a) = auto {
            attrs["auto"] = serde_json::json!(a);
        }
        if !init_sql.is_empty() {
            attrs["init_sql"] = serde_json::json!(init_sql);
        }
        ModelAnalysisInfo {
            model_name: "x.report".to_string(),
            class_name: "XReport".to_string(),
            inherit_from: vec![],
            is_new_model: true,
            docstring: None,
            attrs: Some(attrs),
            fields: vec![],
            methods: vec![],
        }
    }

    #[test]
    fn test_auto_false_create_table_is_warning() {
        let m = model(
            Some(false),
            &["CREATE TABLE IF NOT EXISTS x_report (id serial)"],
        );
        let found = analyze_models(&[m]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "migration-sql-table");
        assert_eq!(found[0].severity, SEVERITY_WARNING);
        assert!(found[0].message.contains("IF NOT EXISTS"));
    }

    #[test]
    fn test_auto_false_create_view_is_info() {
        let m = model(
            Some(false),
            &["CREATE OR REPLACE VIEW x_report AS (SELECT 1)"],
        );
        let found = analyze_models(&[m]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "migration-sql-view");
        assert_eq!(found[0].severity, SEVERITY_INFO);
    }

    #[test]
    fn test_auto_true_or_missing_is_clean() {
        assert!(analyze_models(&[model(Some(true), &["CREATE TABLE t (id int)"])]).is_empty());
        assert!(analyze_models(&[model(None, &["CREATE TABLE t (id int)"])]).is_empty());
        // _auto=False with no init() SQL at all: nothing to flag.
        assert!(analyze_models(&[model(Some(false), &[])]).is_empty());
    }

    #[test]
    fn test_track_visibility_field() {
        let mut m = model(None, &[]);
        m.fields.push(FieldAnalysisInfo {
            name: "stage_id".to_string(),
            field_type: "Many2one".to_string(),
            relation: Some("x.stage".to_string()),
            attrs: Some(serde_json::json!({"track_visibility": "'onchange'"})),
        });
        let found = analyze_models(&[m]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "migration-track-visibility");
        assert_eq!(found[0].context.as_deref(), Some("x.report.stage_id"));
    }

    fn fact(kind: &str, context: Option<&str>, detail: Option<&str>) -> MigrationFactInfo {
        MigrationFactInfo {
            kind: kind.to_string(),
            context: context.map(str::to_string),
            detail: detail.map(str::to_string),
        }
    }

    #[test]
    fn test_facts_map_to_expected_codes() {
        let facts = vec![
            fact("old_api_base", Some("MyModel"), Some("osv.osv")),
            fact("old_style_field_dict", Some("MyModel"), Some("_columns")),
            fact("openerp_import", None, Some("openerp.osv")),
            fact("workflow_call", None, Some("workflow.trg_validate")),
            fact("raw_sql_write", None, Some("UPDATE res_partner SET x = 1")),
            fact("view_tree_tag", Some("view_x_tree"), None),
            fact("view_attrs_states", Some("view_x_form"), None),
            fact("view_t_raw", Some("portal_my_template"), None),
            fact("openerp_manifest", None, None),
            fact("unknown_kind", None, None),
        ];
        let found = analyze_facts(&facts);
        let codes: Vec<&str> = found.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(
            codes,
            vec![
                "migration-old-api-base",
                "migration-old-style-fields",
                "migration-openerp-import",
                "migration-workflow",
                "migration-raw-sql-write",
                "migration-view-tree-tag",
                "migration-view-attrs-states",
                "migration-template-t-raw",
                "migration-openerp-manifest",
            ]
        );
        // Unrecognized kinds are silently skipped, not passed through.
        assert_eq!(found.len(), facts.len() - 1);
    }
}
