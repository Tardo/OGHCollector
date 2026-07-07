// Copyright Alexandre D. Díaz
//! Static security checks over a module's analyzed records (ir.model.access
//! rows from CSV/XML and ir.rule records) and HTTP controllers. Grave
//! findings ("error") are shown on the module detail page; the rest
//! ("warning") only go to the system event log (see main.rs).
//!
//! Odoo-version handling: what actually varies across versions is the xml_id
//! of the portal group (`portal.group_portal` on Odoo <= 11,
//! `base.group_portal` since 12). Matching every historical variant instead
//! of branching on the collected version keeps the checks version-proof: an
//! old id can't collide with a legit internal group on a newer version.

use sqlitedb::models::module_code_analysis::{ControllerAnalysisInfo, RecordAnalysisInfo};
use sqlitedb::models::module_security_warning::{
    SecurityWarningInfo, SEVERITY_ERROR, SEVERITY_WARNING,
};

const PUBLIC_GROUP_XML_IDS: [&str; 3] = [
    "base.group_public",
    "base.group_portal",
    "portal.group_portal",
];

// Write access to any of these models lets a user grant themselves (or
// anyone) further permissions - privilege escalation unless it is reserved
// to the admin groups below.
const PRIVILEGED_MODEL_XML_IDS: [&str; 6] = [
    "model_res_users",
    "model_res_groups",
    "model_ir_rule",
    "model_ir_model_access",
    "model_ir_model",
    "model_ir_model_fields",
];
const ADMIN_GROUP_XML_IDS: [&str; 2] = ["base.group_system", "base.group_erp_manager"];

/// Field lookup tolerant to both sources: XML records store the plain field
/// name ("group_id"), CSV rows keep the raw header, which carries the `:id`
/// suffix for reference columns ("group_id:id").
fn field_str<'a>(rec: &'a RecordAnalysisInfo, name: &str) -> Option<&'a str> {
    let obj = rec.fields.as_ref()?.as_object()?;
    obj.get(name)
        .or_else(|| obj.get(&format!("{name}:id")))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// `"ref('base.group_user')"` (XML repr) -> `base.group_user`; CSV values
/// come through untouched.
fn strip_ref(value: &str) -> &str {
    value
        .strip_prefix("ref('")
        .and_then(|v| v.strip_suffix("')"))
        .or_else(|| {
            value
                .strip_prefix("ref(\"")
                .and_then(|v| v.strip_suffix("\")"))
        })
        .unwrap_or(value)
}

fn perm(rec: &RecordAnalysisInfo, name: &str) -> bool {
    matches!(field_str(rec, name), Some("1" | "true" | "True"))
}

fn granted_write_perms(rec: &RecordAnalysisInfo) -> String {
    let mut out = Vec::new();
    for (col, label) in [
        ("perm_write", "write"),
        ("perm_create", "create"),
        ("perm_unlink", "unlink"),
    ] {
        if perm(rec, col) {
            out.push(label);
        }
    }
    out.join("/")
}

/// True for a domain that matches every record: `[]` or anything containing
/// the classic `(1, '=', 1)` leaf, whatever the spacing/quoting.
fn is_permissive_domain(domain: Option<&str>) -> bool {
    let Some(domain) = domain else { return false };
    let norm: String = domain
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '"' { '\'' } else { c })
        .collect();
    norm == "[]" || norm.contains("(1,'=',1)")
}

fn warning(
    rec: &RecordAnalysisInfo,
    severity: &str,
    code: &str,
    message: String,
) -> SecurityWarningInfo {
    SecurityWarningInfo {
        severity: severity.to_string(),
        code: code.to_string(),
        message,
        xml_id: Some(rec.xml_id.clone()),
    }
}

fn check_access(rec: &RecordAnalysisInfo, out: &mut Vec<SecurityWarningInfo>) {
    let group = field_str(rec, "group_id").map(strip_ref);
    let model_ref = field_str(rec, "model_id").map(strip_ref);
    let model_label = model_ref.unwrap_or("?");
    let write_perms = granted_write_perms(rec);

    match group {
        None => {
            if !write_perms.is_empty() {
                out.push(warning(
                    rec,
                    SEVERITY_ERROR,
                    "acl-global-write",
                    format!(
                        "Access rule grants {write_perms} on '{model_label}' to EVERY user (portal/public included): no group is set"
                    ),
                ));
            } else if perm(rec, "perm_read") {
                out.push(warning(
                    rec,
                    SEVERITY_WARNING,
                    "acl-global-read",
                    format!(
                        "Access rule grants read on '{model_label}' to every user (portal/public included): no group is set"
                    ),
                ));
            }
        }
        Some(g) if PUBLIC_GROUP_XML_IDS.contains(&g) && !write_perms.is_empty() => {
            out.push(warning(
                rec,
                SEVERITY_ERROR,
                "acl-public-write",
                format!(
                    "Access rule grants {write_perms} on '{model_label}' to the portal/public group '{g}'"
                ),
            ));
        }
        _ => {}
    }

    // Independent of the checks above: write access to a security model is
    // an escalation vector for any group that isn't already admin.
    if !write_perms.is_empty() {
        if let Some(model_ref) = model_ref {
            let local_id = model_ref.rsplit('.').next().unwrap_or(model_ref);
            let group_is_admin = matches!(group, Some(g) if ADMIN_GROUP_XML_IDS.contains(&g));
            if PRIVILEGED_MODEL_XML_IDS.contains(&local_id) && !group_is_admin {
                out.push(warning(
                    rec,
                    SEVERITY_ERROR,
                    "acl-privilege-escalation",
                    format!(
                        "Access rule grants {write_perms} on security model '{model_ref}' to {}: members can escalate their own permissions",
                        group.map(|g| format!("group '{g}'")).unwrap_or_else(|| "every user".to_string())
                    ),
                ));
            }
        }
    }
}

fn check_rule(rec: &RecordAnalysisInfo, out: &mut Vec<SecurityWarningInfo>) {
    if !is_permissive_domain(field_str(rec, "domain_force")) {
        return;
    }
    // Group rules OR-combine, so an always-true group rule grants that group
    // access to every record, bypassing sibling rules. A *global* one
    // AND-combines and is a harmless no-op - not reported.
    match field_str(rec, "groups") {
        Some(groups) if PUBLIC_GROUP_XML_IDS.iter().any(|g| groups.contains(g)) => {
            out.push(warning(
                rec,
                SEVERITY_ERROR,
                "rule-public-bypass",
                "Record rule with an always-true domain grants portal/public users access to every record of its model".to_string(),
            ));
        }
        Some(_) => {
            out.push(warning(
                rec,
                SEVERITY_WARNING,
                "rule-group-bypass",
                "Record rule with an always-true domain bypasses every other record rule of its model for its group".to_string(),
            ));
        }
        None => {}
    }
}

/// Computes every security finding for one module from its analyzed records.
pub fn analyze_records(records: &[RecordAnalysisInfo]) -> Vec<SecurityWarningInfo> {
    let mut out = Vec::new();
    for rec in records {
        match rec.model.as_str() {
            "ir.model.access" => check_access(rec, &mut out),
            "ir.rule" => check_rule(rec, &mut out),
            _ => {}
        }
    }
    out
}

fn controller_warning(
    ctrl: &ControllerAnalysisInfo,
    severity: &str,
    code: &str,
    message: String,
) -> SecurityWarningInfo {
    let source = if ctrl.routes.is_empty() {
        format!("{}.{}", ctrl.class_name, ctrl.name)
    } else {
        ctrl.routes.join(", ")
    };
    SecurityWarningInfo {
        severity: severity.to_string(),
        code: code.to_string(),
        message,
        xml_id: Some(source),
    }
}

/// Security findings over the module's HTTP endpoints. Deliberate severity
/// calls: CSRF disabled on an *authenticated* HTTP endpoint is grave (a
/// malicious page can act with the victim's session); disabled on a
/// public/none one is the normal webhook pattern and `.sudo()` inside a
/// public route is extremely common in website modules - both are real
/// review signals but not definite holes, so they only reach the log.
pub fn analyze_controllers(controllers: &[ControllerAnalysisInfo]) -> Vec<SecurityWarningInfo> {
    let mut out = Vec::new();
    for ctrl in controllers {
        let auth = ctrl.auth.as_deref();
        let is_public = matches!(auth, Some("public" | "none"));
        // csrf only matters for type="http" (json routes aren't CSRF-checked
        // the same way) and for state-changing methods; an empty `methods`
        // list means the route accepts every method, POST included.
        let csrf_relevant = ctrl.http_type == "http"
            && (ctrl.methods.is_empty() || ctrl.methods.iter().any(|m| m != "GET"));
        if ctrl.csrf == Some(false) && csrf_relevant {
            if auth == Some("user") {
                out.push(controller_warning(
                    ctrl,
                    SEVERITY_ERROR,
                    "route-user-csrf-off",
                    format!(
                        "HTTP endpoint '{}.{}' disables CSRF protection while requiring an authenticated session: a malicious page can act on behalf of the logged-in user",
                        ctrl.class_name, ctrl.name
                    ),
                ));
            } else if is_public {
                out.push(controller_warning(
                    ctrl,
                    SEVERITY_WARNING,
                    "route-public-csrf-off",
                    format!(
                        "Public HTTP endpoint '{}.{}' disables CSRF protection (fine for webhooks/callbacks, worth a review otherwise)",
                        ctrl.class_name, ctrl.name
                    ),
                ));
            }
        }
        if is_public && ctrl.uses_sudo {
            out.push(controller_warning(
                ctrl,
                SEVERITY_WARNING,
                "route-public-sudo",
                format!(
                    "Unauthenticated endpoint '{}.{}' (auth=\"{}\") calls .sudo(): privileged code reachable without login, review what it exposes",
                    ctrl.class_name,
                    ctrl.name,
                    auth.unwrap_or("?")
                ),
            ));
        }
        if auth == Some("none") {
            out.push(controller_warning(
                ctrl,
                SEVERITY_WARNING,
                "route-auth-none",
                format!(
                    "Endpoint '{}.{}' uses auth=\"none\": it runs with no user/session at all",
                    ctrl.class_name, ctrl.name
                ),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access_csv(group: &str, perms: [&str; 4]) -> RecordAnalysisInfo {
        let mut fields = serde_json::json!({
            "name": "acl",
            "model_id:id": "model_res_partner",
            "perm_read": perms[0],
            "perm_write": perms[1],
            "perm_create": perms[2],
            "perm_unlink": perms[3],
        });
        if !group.is_empty() {
            fields["group_id:id"] = serde_json::json!(group);
        }
        RecordAnalysisInfo {
            xml_id: "acl_test".to_string(),
            model: "ir.model.access".to_string(),
            noupdate: false,
            fields: Some(fields),
        }
    }

    #[test]
    fn test_global_write_is_grave() {
        let found = analyze_records(&[access_csv("", ["1", "1", "0", "0"])]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "acl-global-write");
        assert_eq!(found[0].severity, SEVERITY_ERROR);
        assert!(found[0].message.contains("write"));
        assert_eq!(found[0].xml_id.as_deref(), Some("acl_test"));
    }

    #[test]
    fn test_global_read_is_minor() {
        let found = analyze_records(&[access_csv("", ["1", "0", "0", "0"])]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "acl-global-read");
        assert_eq!(found[0].severity, SEVERITY_WARNING);
    }

    #[test]
    fn test_normal_group_acl_is_clean() {
        let found = analyze_records(&[access_csv("base.group_user", ["1", "1", "1", "1"])]);
        assert!(found.is_empty());
    }

    #[test]
    fn test_portal_write_is_grave_and_old_portal_id_also_matches() {
        for group in [
            "base.group_portal",
            "portal.group_portal",
            "base.group_public",
        ] {
            let found = analyze_records(&[access_csv(group, ["1", "1", "0", "0"])]);
            assert_eq!(found.len(), 1, "group {group}");
            assert_eq!(found[0].code, "acl-public-write");
            assert_eq!(found[0].severity, SEVERITY_ERROR);
        }
        // Read-only portal access is a normal pattern, not a finding.
        let found = analyze_records(&[access_csv("base.group_portal", ["1", "0", "0", "0"])]);
        assert!(found.is_empty());
    }

    #[test]
    fn test_privilege_escalation_from_xml_record() {
        // XML-shaped record: plain field names, ref(...) reprs, "True" evals.
        let rec = RecordAnalysisInfo {
            xml_id: "access_users_hr".to_string(),
            model: "ir.model.access".to_string(),
            noupdate: false,
            fields: Some(serde_json::json!({
                "model_id": "ref('base.model_res_groups')",
                "group_id": "ref('hr.group_hr_user')",
                "perm_read": "True",
                "perm_write": "True",
            })),
        };
        let found = analyze_records(&[rec]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "acl-privilege-escalation");
        assert_eq!(found[0].severity, SEVERITY_ERROR);

        // Same grant reserved to admins: fine.
        let rec_admin = RecordAnalysisInfo {
            xml_id: "access_users_admin".to_string(),
            model: "ir.model.access".to_string(),
            noupdate: false,
            fields: Some(serde_json::json!({
                "model_id": "ref('base.model_res_groups')",
                "group_id": "ref('base.group_system')",
                "perm_write": "True",
            })),
        };
        assert!(analyze_records(&[rec_admin]).is_empty());
    }

    fn rule(domain: &str, groups: Option<&str>) -> RecordAnalysisInfo {
        let mut fields = serde_json::json!({ "domain_force": domain });
        if let Some(g) = groups {
            fields["groups"] = serde_json::json!(g);
        }
        RecordAnalysisInfo {
            xml_id: "rule_test".to_string(),
            model: "ir.rule".to_string(),
            noupdate: false,
            fields: Some(fields),
        }
    }

    #[test]
    fn test_permissive_rule_severity_depends_on_groups() {
        // Portal group + always-true domain: grave.
        let found = analyze_records(&[rule(
            "[(1, '=', 1)]",
            Some("[(4, ref('base.group_portal'))]"),
        )]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "rule-public-bypass");
        assert_eq!(found[0].severity, SEVERITY_ERROR);

        // Internal group: minor (log-only). Also covers the `[]` domain form.
        let found = analyze_records(&[rule(
            "[]",
            Some("[(4, ref('sales_team.group_sale_manager'))]"),
        )]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "rule-group-bypass");
        assert_eq!(found[0].severity, SEVERITY_WARNING);

        // Global always-true rule AND-combines: harmless no-op.
        assert!(analyze_records(&[rule("[(1,'=',1)]", None)]).is_empty());
        // Restrictive rule: clean.
        assert!(analyze_records(&[rule(
            "[('user_id', '=', user.id)]",
            Some("[(4, ref('base.group_portal'))]")
        )])
        .is_empty());
    }

    fn route(
        auth: Option<&str>,
        csrf: Option<bool>,
        methods: &[&str],
        sudo: bool,
    ) -> ControllerAnalysisInfo {
        ControllerAnalysisInfo {
            class_name: "Main".to_string(),
            name: "endpoint".to_string(),
            routes: vec!["/demo/endpoint".to_string()],
            auth: auth.map(str::to_string),
            http_type: "http".to_string(),
            methods: methods.iter().map(|m| m.to_string()).collect(),
            csrf,
            website: false,
            uses_sudo: sudo,
            signature: "(self)".to_string(),
            docstring: None,
        }
    }

    #[test]
    fn test_user_csrf_off_is_grave_public_is_minor() {
        let found = analyze_controllers(&[route(Some("user"), Some(false), &["POST"], false)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "route-user-csrf-off");
        assert_eq!(found[0].severity, SEVERITY_ERROR);
        assert_eq!(found[0].xml_id.as_deref(), Some("/demo/endpoint"));

        let found = analyze_controllers(&[route(Some("public"), Some(false), &["POST"], false)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "route-public-csrf-off");
        assert_eq!(found[0].severity, SEVERITY_WARNING);

        // GET-only route: csrf=False is irrelevant, no finding.
        assert!(
            analyze_controllers(&[route(Some("user"), Some(false), &["GET"], false)]).is_empty()
        );
        // csrf untouched (framework default): clean.
        assert!(analyze_controllers(&[route(Some("user"), None, &["POST"], false)]).is_empty());
    }

    #[test]
    fn test_public_sudo_and_auth_none_are_minor() {
        let found = analyze_controllers(&[route(Some("public"), None, &[], true)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "route-public-sudo");
        assert_eq!(found[0].severity, SEVERITY_WARNING);

        // auth="none" + sudo: both signals reported.
        let found = analyze_controllers(&[route(Some("none"), None, &[], true)]);
        let codes: Vec<&str> = found.iter().map(|w| w.code.as_str()).collect();
        assert_eq!(codes, vec!["route-public-sudo", "route-auth-none"]);

        // sudo behind an authenticated route: normal, clean.
        assert!(analyze_controllers(&[route(Some("user"), None, &[], true)]).is_empty());
        // Unknown auth (inherited-route override): no guessing, clean.
        assert!(analyze_controllers(&[route(None, None, &[], true)]).is_empty());
    }
}
