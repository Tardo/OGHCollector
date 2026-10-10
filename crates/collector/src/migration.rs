// Copyright Alexandre D. Díaz
//! Static "what to check before/after upgrading this module to a newer Odoo
//! version" findings, mirroring security.rs's shape (severity/code/message)
//! but scoped to compatibility/maintainability rather than security. Facts
//! are extracted by analyzer.rs's embedded Python (kept a pure extractor, no
//! judgement); this module turns them into findings, same split security.rs
//! uses for the records/controllers it judges.
//!
//! Version-aware: each API change knows the Odoo version it happened in
//! (checked against Odoo's own source for that version, see `change`), so a
//! module below it gets "Migrating to X.0: ..." and one at/after it gets
//! "Since X.0: ..." (code that is already ignored, broken or deprecated
//! there). Notes are then folded per (code, file): one file full of `<tree>`
//! views is one line, not twenty.
use std::collections::HashMap;

use sqlitedb::models::module_code_analysis::{
    MigrationFactInfo, ModelAnalysisInfo, ModuleAnalysisInfo, RecordAnalysisInfo,
};
use sqlitedb::models::module_migration_note::{
    MigrationConsiderationInfo, SEVERITY_INFO, SEVERITY_WARNING,
};

fn note(
    severity: &str,
    code: &str,
    context: Option<String>,
    message: String,
    file: Option<String>,
    line: Option<i32>,
) -> MigrationConsiderationInfo {
    MigrationConsiderationInfo {
        severity: severity.to_string(),
        code: code.to_string(),
        message,
        context,
        file,
        line,
    }
}

/// (version it changed in, x10 like `version_odoo`; whether the old code is
/// ignored/rejected from then on rather than merely deprecated; what to do).
fn change(kind: &str) -> Option<(u8, bool, &'static str)> {
    Some(match kind {
        "openerp_import" => (100, false, "the `openerp` namespace was renamed to `odoo`; only a deprecated `openerp.addons` alias remained."),
        "workflow_call" => (110, true, "the workflow engine (`trg_validate`) was removed: rewrite as state-field transitions."),
        "api_multi_one" => (130, true, "`@api.multi`/`@api.one` were removed (multi is the default): drop them, looping over `self` where `@api.one` was used."),
        "track_visibility" => (130, true, "`track_visibility` was replaced by `tracking`; the old kwarg is ignored."),
        "view_t_raw" => (150, false, "QWeb `t-raw` is deprecated: use `t-out`; for intended HTML, sanitize it at its source, since `Markup` alone does not sanitize."),
        "fields_view_get" => (160, true, "the web client no longer calls `fields_view_get`: override `_get_view` instead."),
        "view_attrs_states" => (170, true, "`attrs=`/`states=` are rejected in views: convert them to Python expressions in `invisible`/`readonly`/`required`, keeping the original AND/OR logic."),
        "name_get" => (170, true, "`name_get` no longer feeds `display_name`: override `_compute_display_name` instead."),
        "view_tree_tag" => (180, true, "the `tree` view type was renamed to `list` (`<list>` roots, `view_mode`)."),
        "user_has_groups" => (180, true, "`user_has_groups()` was removed: use `self.env.user.has_group()`."),
        "group_operator" => (180, false, "the field kwarg `group_operator` was renamed to `aggregator`."),
        "cron_numbercall" => (180, true, "`ir.cron` lost `numbercall`/`doall`: remove them from the record, data loading fails on unknown fields."),
        "openerp_manifest" => (190, true, "only `__manifest__.py` is recognized: a module with `__openerp__.py` is not detected at all."),
        "sql_constraints" => (190, true, "`_sql_constraints` is ignored and its constraints are silently not created: declare `models.Constraint` attributes instead."),
        _ => return None,
    })
}

fn versioned(
    version_odoo: u8,
    kind: &str,
    context: Option<String>,
    extra: &str,
    file: Option<String>,
    line: Option<i32>,
) -> Option<MigrationConsiderationInfo> {
    let (since, breaks, what) = change(kind)?;
    let when = if version_odoo < since {
        "Migrating to"
    } else {
        "Since"
    };
    Some(note(
        if breaks {
            SEVERITY_WARNING
        } else {
            SEVERITY_INFO
        },
        &format!("migration-{}", kind.replace('_', "-")),
        context,
        format!("{when} {}.0: {what}{extra}", since / 10),
        file,
        line,
    ))
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

/// Every migration consideration for one analyzed module at `version_odoo`.
pub fn analyze_module(
    version_odoo: u8,
    analysis: &ModuleAnalysisInfo,
) -> Vec<MigrationConsiderationInfo> {
    let mut notes = analyze_models(version_odoo, &analysis.models);
    notes.extend(analyze_facts(version_odoo, &analysis.migration_facts));
    notes.extend(analyze_records(version_odoo, &analysis.records));
    // Fold repeats of one code within one file into its first hit.
    let mut out: Vec<MigrationConsiderationInfo> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    let mut index: HashMap<(String, String), usize> = HashMap::new();
    for n in notes {
        if let Some(file) = &n.file {
            if let Some(&i) = index.get(&(n.code.clone(), file.clone())) {
                counts[i] += 1;
                continue;
            }
            index.insert((n.code.clone(), file.clone()), out.len());
        }
        out.push(n);
        counts.push(1);
    }
    for (n, count) in out.iter_mut().zip(counts) {
        if count > 1 {
            n.message
                .push_str(&format!(" ({count} occurrences in this file)"));
        }
    }
    out
}

/// `ir.cron` records still setting the fields removed in 18.0.
pub fn analyze_records(
    version_odoo: u8,
    records: &[RecordAnalysisInfo],
) -> Vec<MigrationConsiderationInfo> {
    records
        .iter()
        .filter(|r| {
            r.model == "ir.cron"
                && attrs_obj(&r.fields)
                    .is_some_and(|f| f.contains_key("numbercall") || f.contains_key("doall"))
        })
        .filter_map(|r| {
            versioned(
                version_odoo,
                "cron_numbercall",
                Some(r.xml_id.clone()),
                "",
                r.file.clone(),
                r.line,
            )
        })
        .collect()
}

/// Findings from a module's analyzed models/fields: `_auto = False` models
/// whose `init()` hand-rolls SQL (a real table vs. the usual SQL view), and
/// renamed field kwargs already captured in `fields[].attrs` (one note per
/// model and kwarg, listing the fields).
pub fn analyze_models(
    version_odoo: u8,
    models: &[ModelAnalysisInfo],
) -> Vec<MigrationConsiderationInfo> {
    let mut out = Vec::new();
    for m in models {
        if attrs_bool(&m.attrs, "auto") == Some(false) {
            let mut seen = std::collections::BTreeSet::new();
            for sql in attrs_str_list(&m.attrs, "init_sql") {
                let upper = sql
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
                    .to_uppercase();
                if upper.starts_with("CREATE TABLE ") && seen.insert("table") {
                    out.push(note(
                        SEVERITY_WARNING,
                        "migration-sql-table",
                        Some(m.model_name.clone()),
                        format!(
                            "Model '{}' has `_auto = False` and its init() creates a real TABLE via raw SQL{}: Odoo's ORM does not manage this table's schema, so column/index changes across versions must be applied by hand (an `IF NOT EXISTS` guard also means an already-existing table silently keeps its old schema after upgrading).",
                            m.model_name,
                            if upper.contains("IF NOT EXISTS") { " (`CREATE TABLE IF NOT EXISTS`)" } else { "" }
                        ),
                        None,
                        None,
                    ));
                } else if (upper.starts_with("CREATE VIEW ")
                    || upper.starts_with("CREATE OR REPLACE VIEW ")
                    || upper.starts_with("CREATE MATERIALIZED VIEW "))
                    && seen.insert("view")
                {
                    out.push(note(
                        SEVERITY_INFO,
                        "migration-sql-view",
                        Some(m.model_name.clone()),
                        format!(
                            "Model '{}' has `_auto = False` and its init() (re)creates a SQL VIEW: the standard reporting-model pattern, but its raw SQL isn't checked by the ORM - verify it against renamed/removed tables and columns in the target version.",
                            m.model_name
                        ),
                        None,
                        None,
                    ));
                }
            }
        }
        for kwarg in ["track_visibility", "group_operator"] {
            let fields: Vec<&str> = m
                .fields
                .iter()
                .filter(|f| attrs_obj(&f.attrs).is_some_and(|o| o.contains_key(kwarg)))
                .map(|f| f.name.as_str())
                .collect();
            if !fields.is_empty() {
                out.extend(versioned(
                    version_odoo,
                    kwarg,
                    Some(m.model_name.clone()),
                    &format!(" Fields: {}.", fields.join(", ")),
                    None,
                    None,
                ));
            }
        }
    }
    out
}

/// Findings from the raw migration_facts list (old-API classes, hand-rolled
/// SQL writes, removed/renamed APIs, deprecated view/QWeb syntax, ...).
pub fn analyze_facts(
    version_odoo: u8,
    facts: &[MigrationFactInfo],
) -> Vec<MigrationConsiderationInfo> {
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
                f.file.clone(),
                f.line,
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
                f.file.clone(),
                f.line,
            )),
            "raw_sql_write" => out.push(note(
                SEVERITY_INFO,
                "migration-raw-sql-write",
                None,
                format!(
                    "Raw `cr.execute()` INSERT/UPDATE/DELETE bypasses the ORM (no compute/constrains/tracking/mail): `{}` - re-check the table/column names still match after upgrading.",
                    f.detail.as_deref().unwrap_or("")
                ),
                f.file.clone(),
                f.line,
            )),
            kind => {
                let extra = match kind {
                    "api_multi_one" => {
                        format!(" ({} methods)", f.detail.as_deref().unwrap_or("?"))
                    }
                    "openerp_import" => {
                        format!(" (`{}`)", f.detail.as_deref().unwrap_or("openerp"))
                    }
                    _ => String::new(),
                };
                out.extend(versioned(
                    version_odoo,
                    kind,
                    ctx,
                    &extra,
                    f.file.clone(),
                    f.line,
                ));
            }
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
        let found = analyze_models(170, &[m]);
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
        let found = analyze_models(170, &[m]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "migration-sql-view");
        assert_eq!(found[0].severity, SEVERITY_INFO);
    }

    #[test]
    fn test_sql_notes_require_ddl_and_do_not_repeat_per_statement() {
        let m = model(
            Some(false),
            &[
                "SELECT preview FROM x_report",
                "DROP VIEW IF EXISTS x_report",
                "CREATE\nTABLE IF NOT EXISTS x_report (id int)",
                "CREATE TABLE IF NOT EXISTS x_other (id int)",
                "CREATE OR REPLACE VIEW x_view AS SELECT 1",
            ],
        );
        let found = analyze_models(170, &[m]);
        assert_eq!(
            found.iter().map(|n| n.code.as_str()).collect::<Vec<_>>(),
            ["migration-sql-table", "migration-sql-view"]
        );
        let found = analyze_facts(170, &[fact("view_t_raw", Some("template"), None)]);
        assert_eq!(found[0].severity, SEVERITY_INFO);
        assert!(found[0].message.contains("does not sanitize"));
    }

    #[test]
    fn test_auto_true_or_missing_is_clean() {
        assert!(analyze_models(170, &[model(Some(true), &["CREATE TABLE t (id int)"])]).is_empty());
        assert!(analyze_models(170, &[model(None, &["CREATE TABLE t (id int)"])]).is_empty());
        // _auto=False with no init() SQL at all: nothing to flag.
        assert!(analyze_models(170, &[model(Some(false), &[])]).is_empty());
    }

    #[test]
    fn test_renamed_field_kwargs_fold_per_model() {
        let mut m = model(None, &[]);
        for (name, kwarg) in [
            ("stage_id", "track_visibility"),
            ("user_id", "track_visibility"),
            ("amount", "group_operator"),
        ] {
            m.fields.push(FieldAnalysisInfo {
                name: name.to_string(),
                field_type: "Many2one".to_string(),
                relation: None,
                attrs: Some(serde_json::json!({ kwarg: "'x'" })),
            });
        }
        let found = analyze_models(120, &[m]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].code, "migration-track-visibility");
        assert_eq!(found[0].context.as_deref(), Some("x.report"));
        assert!(found[0].message.starts_with("Migrating to 13.0:"));
        assert!(found[0].message.ends_with("Fields: stage_id, user_id."));
        // Deprecated-only rename: info, not warning.
        assert_eq!(found[1].code, "migration-group-operator");
        assert_eq!(found[1].severity, SEVERITY_INFO);
    }

    fn fact(kind: &str, context: Option<&str>, detail: Option<&str>) -> MigrationFactInfo {
        MigrationFactInfo {
            kind: kind.to_string(),
            context: context.map(str::to_string),
            detail: detail.map(str::to_string),
            file: Some("models/x.py".to_string()),
            line: Some(12),
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
            fact("api_multi_one", Some("MyModel"), Some("3")),
            fact("fields_view_get", Some("MyModel"), None),
            fact("name_get", Some("MyModel"), None),
            fact("user_has_groups", None, Some("self.user_has_groups")),
            fact("sql_constraints", Some("MyModel"), None),
            fact("unknown_kind", None, None),
        ];
        let found = analyze_facts(90, &facts);
        let codes: Vec<&str> = found.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(
            codes,
            vec![
                "migration-old-api-base",
                "migration-old-style-fields",
                "migration-openerp-import",
                "migration-workflow-call",
                "migration-raw-sql-write",
                "migration-view-tree-tag",
                "migration-view-attrs-states",
                "migration-view-t-raw",
                "migration-openerp-manifest",
                "migration-api-multi-one",
                "migration-fields-view-get",
                "migration-name-get",
                "migration-user-has-groups",
                "migration-sql-constraints",
            ]
        );
        // Unrecognized kinds are silently skipped, not passed through.
        assert_eq!(found.len(), facts.len() - 1);
        // file/line ride along from the fact into the finding untouched.
        assert_eq!(found[0].file.as_deref(), Some("models/x.py"));
        assert_eq!(found[0].line, Some(12));
        assert!(found[9].message.contains("(3 methods)"));
    }

    #[test]
    fn test_version_decides_wording_not_relevance() {
        // A 16.0 module still using attrs=: a to-do for its next migration.
        let found = analyze_facts(160, &[fact("view_attrs_states", Some("v"), None)]);
        assert_eq!(found[0].severity, SEVERITY_WARNING);
        assert!(found[0].message.starts_with("Migrating to 17.0:"));
        // The same view in a 17.0 module: already rejected on its own version.
        let found = analyze_facts(170, &[fact("view_attrs_states", Some("v"), None)]);
        assert!(found[0].message.starts_with("Since 17.0:"));
        // ir.cron's numbercall: a record-level rule, 18.0.
        let cron = RecordAnalysisInfo {
            xml_id: "cron_x".to_string(),
            model: "ir.cron".to_string(),
            fields: Some(serde_json::json!({"numbercall": "-1"})),
            ..Default::default()
        };
        let found = analyze_records(170, std::slice::from_ref(&cron));
        assert_eq!(found[0].code, "migration-cron-numbercall");
        assert!(found[0].message.starts_with("Migrating to 18.0:"));
        assert!(analyze_records(
            170,
            &[RecordAnalysisInfo {
                fields: None,
                ..cron
            }]
        )
        .is_empty());
    }

    #[test]
    fn test_analyze_module_folds_repeats_per_file() {
        let mut views = fact("view_tree_tag", Some("view_a"), None);
        views.file = Some("views/a.xml".to_string());
        let analysis = ModuleAnalysisInfo {
            migration_facts: vec![
                views.clone(),
                MigrationFactInfo {
                    context: Some("view_b".to_string()),
                    line: Some(40),
                    ..views.clone()
                },
                MigrationFactInfo {
                    file: Some("views/b.xml".to_string()),
                    ..views
                },
            ],
            ..Default::default()
        };
        let found = analyze_module(170, &analysis);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].context.as_deref(), Some("view_a"));
        assert!(found[0].message.ends_with("(2 occurrences in this file)"));
        assert!(!found[1].message.contains("occurrences"));
    }
}
