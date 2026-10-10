// Copyright Alexandre D. Díaz
//! What changed in a module between two of its rows - typically the same
//! module at two Odoo versions: manifest, direct dependencies and the stored
//! code analysis (models/fields/methods, views, HTTP routes, security
//! finding counts). Shared by the server's compare page/API and the MCP
//! `compare_module_versions` tool. Records (`module_record`) are left out on
//! purpose: localization data files make them huge and noisy.
use diesel::sqlite::SqliteConnection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use super::{
    dependency, dependency_module, dependency_type, module, module_controller, module_maintainer,
    module_model, module_model_field, module_model_method, module_security_warning, module_version,
    module_view,
};
use oghutils::version::odoo_version_u8_to_string;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedValue {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValueChange {
    pub name: String,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemDiff {
    pub added: Vec<NamedValue>,
    pub removed: Vec<NamedValue>,
    pub changed: Vec<ValueChange>,
}

impl ItemDiff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDiff {
    pub model: String,
    /// "added", "removed" or "changed".
    pub status: String,
    /// Field name -> "type" or "type → comodel".
    pub fields: ItemDiff,
    /// Public method name -> signature.
    pub methods: ItemDiff,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SecurityCounts {
    pub grave: usize,
    pub minor: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleDiff {
    pub technical_name: String,
    pub from_odoo_version: String,
    pub to_odoo_version: String,
    pub from_module_version: String,
    pub to_module_version: String,
    /// Manifest values that differ (name, license, category, flags,
    /// maintainers).
    pub manifest: Vec<ValueChange>,
    /// Direct Odoo `depends`.
    pub depends: ListDiff,
    pub pip: ListDiff,
    pub bin: ListDiff,
    /// False when either side has no stored code analysis: `models`,
    /// `views` and `routes` are then left empty instead of reporting
    /// everything as added or removed.
    pub analysis_available: bool,
    /// Only models with changes.
    pub models: Vec<ModelDiff>,
    /// View xml_id -> "type on model[, inherits xml_id]".
    pub views: ItemDiff,
    /// HTTP route path -> "auth=…, type=…".
    pub routes: ItemDiff,
    pub security_from: Option<SecurityCounts>,
    pub security_to: Option<SecurityCounts>,
}

#[derive(Default)]
struct ModelSnapshot {
    fields: BTreeMap<String, String>,
    methods: BTreeMap<String, String>,
}

struct Analysis {
    models: BTreeMap<String, ModelSnapshot>,
    views: BTreeMap<String, String>,
    routes: BTreeMap<String, String>,
    security: SecurityCounts,
}

struct Snapshot {
    odoo_version: String,
    module_version: String,
    manifest: Vec<(&'static str, String)>,
    depends: BTreeSet<String>,
    pip: BTreeSet<String>,
    bin: BTreeSet<String>,
    analysis: Option<Analysis>,
}

fn load_analysis(conn: &mut SqliteConnection, module_version_id: &i64) -> Analysis {
    // A model can be extended by several classes of the same module: merge
    // them by model name.
    let mut models: BTreeMap<String, ModelSnapshot> = BTreeMap::new();
    for m in module_model::get_by_module_version_id(conn, module_version_id) {
        let fields = module_model_field::get_by_module_model_id(conn, &m.id);
        let methods = module_model_method::get_by_module_model_id(conn, &m.id);
        let snapshot = models.entry(m.model_name).or_default();
        for f in fields {
            let value = match f.relation {
                Some(relation) => format!("{} → {relation}", f.field_type),
                None => f.field_type,
            };
            snapshot.fields.insert(f.name, value);
        }
        for meth in methods {
            snapshot.methods.insert(meth.name, meth.signature);
        }
    }
    let views = module_view::get_by_module_version_id(conn, module_version_id)
        .into_iter()
        .map(|v| {
            let mut value = format!(
                "{} on {}",
                v.view_type.as_deref().unwrap_or("view"),
                v.model.as_deref().unwrap_or("?")
            );
            if let Some(inherit) = v.inherit_xml_id {
                value.push_str(&format!(", inherits {inherit}"));
            }
            (v.xml_id, value)
        })
        .collect();
    let mut routes = BTreeMap::new();
    for c in module_controller::get_by_module_version_id(conn, module_version_id) {
        let value = format!(
            "auth={}, type={}",
            c.auth.as_deref().unwrap_or("inherited"),
            c.http_type
        );
        for route in c.routes_vec() {
            routes.insert(route, value.clone());
        }
    }
    let mut security = SecurityCounts::default();
    for w in module_security_warning::get_by_module_version_id(conn, module_version_id) {
        if w.severity == module_security_warning::SEVERITY_ERROR {
            security.grave += 1;
        } else if w.severity == module_security_warning::SEVERITY_WARNING {
            security.minor += 1;
        }
    }
    Analysis {
        models,
        views,
        routes,
        security,
    }
}

fn load_snapshot(
    conn: &mut SqliteConnection,
    m: &module::Model,
    module_dep_type_id: &i64,
) -> Snapshot {
    let manifest = vec![
        ("name", m.name.clone()),
        ("license", m.license_str().to_string()),
        ("category", m.category_str().to_string()),
        ("application", m.application.to_string()),
        ("installable", m.installable.to_string()),
        ("auto_install", m.auto_install.to_string()),
        (
            "maintainers",
            module_maintainer::get_names_by_module_id(conn, &m.id).join(", "),
        ),
    ];
    let depends = dependency_module::get_names(conn, &m.id, module_dep_type_id)
        .into_iter()
        .collect();
    let pip = dependency::get_module_external_dependency_names(conn, &m.id, "python")
        .into_iter()
        .collect();
    let bin = dependency::get_module_external_dependency_names(conn, &m.id, "bin")
        .into_iter()
        .collect();
    let analysis = module_version::resolve_current(conn, m).map(|mv| load_analysis(conn, &mv.id));
    Snapshot {
        odoo_version: odoo_version_u8_to_string(&(m.version_odoo as u8)),
        module_version: m.version_module.clone(),
        manifest,
        depends,
        pip,
        bin,
        analysis,
    }
}

fn diff_set(from: &BTreeSet<String>, to: &BTreeSet<String>) -> ListDiff {
    ListDiff {
        added: to.difference(from).cloned().collect(),
        removed: from.difference(to).cloned().collect(),
    }
}

fn diff_map(from: &BTreeMap<String, String>, to: &BTreeMap<String, String>) -> ItemDiff {
    let named = |(name, value): (&String, &String)| NamedValue {
        name: name.clone(),
        value: value.clone(),
    };
    ItemDiff {
        added: to
            .iter()
            .filter(|(k, _)| !from.contains_key(*k))
            .map(named)
            .collect(),
        removed: from
            .iter()
            .filter(|(k, _)| !to.contains_key(*k))
            .map(named)
            .collect(),
        changed: from
            .iter()
            .filter_map(|(name, old)| {
                let new = to.get(name)?;
                (new != old).then(|| ValueChange {
                    name: name.clone(),
                    from: old.clone(),
                    to: new.clone(),
                })
            })
            .collect(),
    }
}

fn diff_models(
    from: &BTreeMap<String, ModelSnapshot>,
    to: &BTreeMap<String, ModelSnapshot>,
) -> Vec<ModelDiff> {
    let empty = ModelSnapshot::default();
    let names: BTreeSet<&String> = from.keys().chain(to.keys()).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let (old, new) = (from.get(name), to.get(name));
            let status = match (old, new) {
                (None, _) => "added",
                (_, None) => "removed",
                _ => "changed",
            };
            let (old, new) = (old.unwrap_or(&empty), new.unwrap_or(&empty));
            let diff = ModelDiff {
                model: name.clone(),
                status: status.to_string(),
                fields: diff_map(&old.fields, &new.fields),
                methods: diff_map(&old.methods, &new.methods),
            };
            (status != "changed" || !diff.fields.is_empty() || !diff.methods.is_empty())
                .then_some(diff)
        })
        .collect()
}

/// Compares two module rows (`from` is the baseline).
pub fn compare(
    conn: &mut SqliteConnection,
    from: &module::Model,
    to: &module::Model,
) -> ModuleDiff {
    // Seeded by the base migration - always present.
    let module_dep_type_id = dependency_type::get_by_name(conn, "module")
        .expect("dependency_type 'module' is seeded by migrations")
        .id;
    let old = load_snapshot(conn, from, &module_dep_type_id);
    let new = load_snapshot(conn, to, &module_dep_type_id);

    let manifest = old
        .manifest
        .iter()
        .zip(&new.manifest)
        .filter(|((_, a), (_, b))| a != b)
        .map(|((name, a), (_, b))| ValueChange {
            name: name.to_string(),
            from: a.clone(),
            to: b.clone(),
        })
        .collect();

    let mut diff = ModuleDiff {
        technical_name: to.technical_name.clone(),
        from_odoo_version: old.odoo_version,
        to_odoo_version: new.odoo_version,
        from_module_version: old.module_version,
        to_module_version: new.module_version,
        manifest,
        depends: diff_set(&old.depends, &new.depends),
        pip: diff_set(&old.pip, &new.pip),
        bin: diff_set(&old.bin, &new.bin),
        security_from: old.analysis.as_ref().map(|a| a.security),
        security_to: new.analysis.as_ref().map(|a| a.security),
        ..Default::default()
    };
    if let (Some(a), Some(b)) = (&old.analysis, &new.analysis) {
        diff.analysis_available = true;
        diff.models = diff_models(&a.models, &b.models);
        diff.views = diff_map(&a.views, &b.views);
        diff.routes = diff_map(&a.routes, &b.routes);
    }
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::module_code_analysis::{
        ControllerAnalysisInfo, FieldAnalysisInfo, MethodAnalysisInfo, ModelAnalysisInfo,
        ViewAnalysisInfo,
    };
    use crate::models::{dependency_module, dependency_type};
    use diesel::Connection;
    use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

    const MIGRATIONS: EmbeddedMigrations = embed_migrations!("../../migrations");

    fn field(name: &str, field_type: &str, relation: Option<&str>) -> FieldAnalysisInfo {
        FieldAnalysisInfo {
            name: name.to_string(),
            field_type: field_type.to_string(),
            relation: relation.map(str::to_string),
            attrs: None,
        }
    }

    fn method(name: &str, signature: &str) -> MethodAnalysisInfo {
        MethodAnalysisInfo {
            name: name.to_string(),
            decorators: vec![],
            signature: signature.to_string(),
            docstring: None,
        }
    }

    fn model(
        name: &str,
        fields: Vec<FieldAnalysisInfo>,
        methods: Vec<MethodAnalysisInfo>,
    ) -> ModelAnalysisInfo {
        ModelAnalysisInfo {
            model_name: name.to_string(),
            class_name: "X".to_string(),
            fields,
            methods,
            ..Default::default()
        }
    }

    fn make(
        conn: &mut SqliteConnection,
        version_odoo: u8,
        license: &str,
        depends: &[&str],
        models: Vec<ModelAnalysisInfo>,
        views: Vec<ViewAnalysisInfo>,
        routes: &[(&str, &str)],
    ) -> module::Model {
        let m = module::add(
            conn,
            &module::ManifestInfo {
                technical_name: "sale_x".to_string(),
                source_path: "sale_x".to_string(),
                version_odoo,
                name: "Sale X".to_string(),
                version_module: "1.0.0".to_string(),
                license: license.to_string(),
                installable: true,
                git_org: "OCA".to_string(),
                git_repo: "sale-workflow".to_string(),
                last_commit_hash: "abc".to_string(),
                last_commit_date: "2024-01-01".to_string(),
                ..Default::default()
            },
        )
        .unwrap();
        let dep_type = dependency_type::get_by_name(conn, "module").unwrap();
        for dep in depends {
            dependency_module::add(conn, &dep_type.id, dep, &m.id).unwrap();
        }
        let mv = module_version::get_or_create(conn, &m.id, &m.version_module).unwrap();
        module_model::replace_for_module(conn, &m.id, &mv.id, &models).unwrap();
        module_view::replace_for_module(conn, &m.id, &mv.id, &views).unwrap();
        let controllers: Vec<ControllerAnalysisInfo> = routes
            .iter()
            .map(|(route, auth)| ControllerAnalysisInfo {
                class_name: "C".to_string(),
                name: "h".to_string(),
                routes: vec![route.to_string()],
                auth: Some(auth.to_string()),
                http_type: "http".to_string(),
                ..Default::default()
            })
            .collect();
        module_controller::replace_for_module(conn, &m.id, &mv.id, &controllers).unwrap();
        m
    }

    fn view(xml_id: &str, model: &str, view_type: &str) -> ViewAnalysisInfo {
        ViewAnalysisInfo {
            xml_id: xml_id.to_string(),
            name: None,
            model: Some(model.to_string()),
            inherit_xml_id: None,
            view_type: Some(view_type.to_string()),
        }
    }

    #[test]
    fn compare_reports_manifest_deps_models_views_and_routes() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.run_pending_migrations(MIGRATIONS).unwrap();
        let old = make(
            &mut conn,
            160,
            "AGPL-3",
            &["sale", "mail"],
            vec![
                model(
                    "sale.order",
                    vec![
                        field("x_type", "Many2one", Some("sale.type")),
                        field("x_old", "Char", None),
                    ],
                    vec![method("action_x", "(self)")],
                ),
                model("sale.type", vec![], vec![]),
            ],
            vec![view("view_a", "sale.order", "tree")],
            &[("/x/a", "user")],
        );
        let new = make(
            &mut conn,
            170,
            "LGPL-3",
            &["sale"],
            vec![
                model(
                    "sale.order",
                    vec![
                        field("x_type", "Many2one", Some("sale.order.type")),
                        field("x_new", "Text", None),
                    ],
                    vec![method("action_x", "(self, force=False)")],
                ),
                // same model extended by a second class: merged
                model("sale.order", vec![field("x_more", "Boolean", None)], vec![]),
                model("sale.type", vec![], vec![]),
            ],
            vec![
                view("view_a", "sale.order", "list"),
                view("view_b", "sale.order", "form"),
            ],
            &[("/x/a", "public"), ("/x/b", "user")],
        );

        let diff = compare(&mut conn, &old, &new);
        assert!(diff.analysis_available);
        assert_eq!(
            (
                diff.from_odoo_version.as_str(),
                diff.to_odoo_version.as_str()
            ),
            ("16.0", "17.0")
        );
        assert_eq!(diff.manifest.len(), 1);
        assert_eq!(
            (
                diff.manifest[0].name.as_str(),
                diff.manifest[0].from.as_str(),
                diff.manifest[0].to.as_str()
            ),
            ("license", "AGPL-3", "LGPL-3")
        );
        assert_eq!(diff.depends.removed, vec!["mail"]);
        assert!(diff.depends.added.is_empty());

        // sale.type is unchanged, so only sale.order is reported.
        assert_eq!(diff.models.len(), 1);
        let so = &diff.models[0];
        assert_eq!(so.status, "changed");
        let names = |v: &[NamedValue]| v.iter().map(|n| n.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&so.fields.added), vec!["x_more", "x_new"]);
        assert_eq!(names(&so.fields.removed), vec!["x_old"]);
        assert_eq!(so.fields.changed[0].to, "Many2one → sale.order.type");
        assert_eq!(so.methods.changed[0].to, "(self, force=False)");

        assert_eq!(names(&diff.views.added), vec!["view_b"]);
        assert_eq!(diff.views.changed[0].from, "tree on sale.order");
        assert_eq!(names(&diff.routes.added), vec!["/x/b"]);
        assert_eq!(diff.routes.changed[0].to, "auth=public, type=http");
    }

    #[test]
    fn compare_without_analysis_on_one_side_skips_code_sections() {
        let mut conn = SqliteConnection::establish(":memory:").unwrap();
        conn.run_pending_migrations(MIGRATIONS).unwrap();
        let old = make(
            &mut conn,
            160,
            "AGPL-3",
            &[],
            vec![model("a.b", vec![], vec![])],
            vec![],
            &[],
        );
        let new = make(&mut conn, 170, "AGPL-3", &[], vec![], vec![], &[]);
        // Drop the 17.0 snapshot row: no analysis stored for that side.
        module_version::delete_by_module_id(&mut conn, &new.id).unwrap();

        let diff = compare(&mut conn, &old, &new);
        assert!(!diff.analysis_available);
        assert!(diff.models.is_empty());
        assert!(diff.security_to.is_none());
    }
}
