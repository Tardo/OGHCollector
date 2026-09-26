// Copyright Alexandre D. Díaz
//! Static security checks over a module's analyzed records (ir.model.access
//! rows from CSV/XML and ir.rule records) and HTTP controllers. Findings
//! identify review priorities, not confirmed exploits. Effective access also
//! depends on other modules, global rules, field restrictions and Python code.
use sqlitedb::models::module_code_analysis::{ControllerAnalysisInfo, RecordAnalysisInfo};
use sqlitedb::models::module_security_warning::{
    SecurityWarningInfo, SEVERITY_ERROR, SEVERITY_WARNING,
};

const PUBLIC_GROUP_XML_IDS: [&str; 2] = ["group_public", "group_portal"];

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
const ADMIN_GROUP_XML_IDS: [&str; 2] = ["group_system", "group_erp_manager"];

fn is_core_id(xml_id: &str, ids: &[&str]) -> bool {
    let local = xml_id.strip_prefix("base.").unwrap_or(xml_id);
    ids.contains(&local) || (xml_id == "portal.group_portal" && ids.contains(&"group_portal"))
}

/// Field lookup tolerant to both sources: XML records store the plain field
/// name ("group_id"), CSV rows keep the raw header, which carries a suffix
/// for reference columns - Odoo accepts both "group_id:id" and "group_id/id"
/// (e.g. addons/lunch uses `/id` while addons/sale uses `:id`).
fn field_str<'a>(rec: &'a RecordAnalysisInfo, name: &str) -> Option<&'a str> {
    let obj = rec.fields.as_ref()?.as_object()?;
    obj.get(name)
        .or_else(|| obj.get(&format!("{name}:id")))
        .or_else(|| obj.get(&format!("{name}/id")))
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

/// True only when the field is *explicitly* disabled - Odoo's `ir.rule`
/// perm_read/write/create/unlink all default to True, so an absent field
/// still grants that operation. Used to tell a genuinely read-only rule
/// (all three explicitly off) apart from one that merely omits them.
fn perm_explicit_false(rec: &RecordAnalysisInfo, name: &str) -> bool {
    matches!(field_str(rec, name), Some("0" | "false" | "False"))
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

/// Recognize only complete, unconditionally true domains. A true leaf inside
/// an AND/NOT expression does not make the whole domain true.
fn is_permissive_domain(domain: Option<&str>) -> bool {
    let Some(domain) = domain else { return false };
    let norm: String = domain
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| if c == '"' { '\'' } else { c })
        .collect();
    matches!(norm.as_str(), "[]" | "[(1,'=',1)]")
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
        file: rec.file.clone(),
        line: rec.line,
    }
}

/// Explicitly named broad access is useful inventory, not an unexpected grant.
/// This does not suppress the independent security-model escalation check.
fn is_intentional_global(rec: &RecordAnalysisInfo) -> bool {
    names_scope(rec, "all") || names_scope(rec, "public")
}

fn names_scope(rec: &RecordAnalysisInfo, scope: &str) -> bool {
    let has_token = |s: &str| s.split(['_', '.']).any(|t| t == scope);
    has_token(&rec.xml_id) || field_str(rec, "name").is_some_and(has_token)
}

fn check_access(rec: &RecordAnalysisInfo, out: &mut Vec<SecurityWarningInfo>) {
    let group = field_str(rec, "group_id").map(strip_ref);
    let model_ref = field_str(rec, "model_id").map(strip_ref);
    let model_label = model_ref.unwrap_or("?");
    let write_perms = granted_write_perms(rec);

    match group {
        None => {
            let intentional = is_intentional_global(rec);
            if !write_perms.is_empty() {
                out.push(warning(
                    rec,
                    if intentional { SEVERITY_WARNING } else { SEVERITY_ERROR },
                    "acl-global-write",
                    format!(
                        "ACL grants {write_perms} on '{model_label}' to every user (portal/public included): no group is set. Verify record rules and test access as an unrelated public/portal user."
                    ),
                ));
            } else if perm(rec, "perm_read") && !intentional {
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
        Some(g) if is_core_id(g, &PUBLIC_GROUP_XML_IDS) && !write_perms.is_empty() => {
            let intentional = is_intentional_global(rec)
                || names_scope(
                    rec,
                    g.rsplit('.')
                        .next()
                        .unwrap_or(g)
                        .trim_start_matches("group_"),
                );
            out.push(warning(
                rec,
                if intentional { SEVERITY_WARNING } else { SEVERITY_ERROR },
                "acl-public-write",
                format!(
                    "ACL grants {write_perms} on '{model_label}' to '{g}'. Verify record rules restrict these operations to intended records, including records owned by other users/companies."
                ),
            ));
        }
        _ => {}
    }

    // Independent of the checks above: write access to a security model is
    // an escalation vector for any group that isn't already admin.
    if !write_perms.is_empty() {
        if let Some(model_ref) = model_ref {
            let group_is_admin = matches!(group, Some(g) if is_core_id(g, &ADMIN_GROUP_XML_IDS));
            if is_core_id(model_ref, &PRIVILEGED_MODEL_XML_IDS) && !group_is_admin {
                out.push(warning(
                    rec,
                    SEVERITY_ERROR,
                    "acl-privilege-escalation",
                    format!(
                        "ACL grants {write_perms} on security model '{model_ref}' to {}. Review field restrictions and Python guards for possible privilege escalation; the ACL alone does not prove exploitability.",
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
    if ["perm_read", "perm_write", "perm_create", "perm_unlink"]
        .iter()
        .all(|p| perm_explicit_false(rec, p))
    {
        return;
    }
    // Group rules OR-combine, but ACLs and global rules still constrain access.
    match field_str(rec, "groups") {
        Some(groups)
            if groups
                .split(['\'', '"'])
                .any(|g| is_core_id(g, &PUBLIC_GROUP_XML_IDS)) =>
        {
            out.push(warning(
                rec,
                SEVERITY_ERROR,
                "rule-public-bypass",
                "Always-true portal/public group rule removes restrictions from other group rules for applicable operations. ACLs and global rules still apply; test access to other users' records.".to_string(),
            ));
        }
        Some("[]" | "False" | "[(5,)]" | "[(6, 0, [])]") => {}
        Some(groups)
            if is_intentional_global(rec)
                || groups.split(['\'', '"']).any(|g| {
                    is_core_id(g, &ADMIN_GROUP_XML_IDS)
                        || g.split(['_', '.'])
                            .any(|t| matches!(t, "manager" | "admin"))
                }) => {}
        Some(_)
            if ["perm_write", "perm_create", "perm_unlink"]
                .iter()
                .all(|p| perm_explicit_false(rec, p)) => {}
        Some(_) => {
            out.push(warning(
                rec,
                SEVERITY_WARNING,
                "rule-group-bypass",
                "Always-true group rule removes restrictions from other group rules for applicable operations. Verify this scope is intended; ACLs and global rules still apply.".to_string(),
            ));
        }
        None => {}
    }
}

/// Computes every security finding for one module from its analyzed records.
pub fn analyze_records(records: &[RecordAnalysisInfo]) -> Vec<SecurityWarningInfo> {
    let mut out = Vec::new();
    for rec in records {
        if perm_explicit_false(rec, "active") {
            continue;
        }
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
        file: ctrl.file.clone(),
        line: ctrl.line,
    }
}

/// A method call's presence is evidence to review, not proof that every path
/// or every record is protected by it.
pub fn analyze_controllers(controllers: &[ControllerAnalysisInfo]) -> Vec<SecurityWarningInfo> {
    let mut out = Vec::new();
    for ctrl in controllers {
        let auth = ctrl.auth.as_deref();
        let is_public = matches!(auth, Some("public" | "none"));
        // csrf only matters for type="http" (json routes aren't CSRF-checked
        // the same way) and for state-changing methods; an empty `methods`
        // list means the route accepts every method, POST included.
        let csrf_relevant = ctrl.http_type == "http"
            && (ctrl.methods.is_empty()
                || ctrl
                    .methods
                    .iter()
                    .any(|m| !matches!(m.as_str(), "GET" | "HEAD" | "OPTIONS" | "TRACE")));
        if ctrl.csrf == Some(false) && csrf_relevant {
            if auth == Some("user") {
                out.push(controller_warning(
                    ctrl,
                    SEVERITY_ERROR,
                    "route-user-csrf-off",
                    format!(
                        "HTTP endpoint '{}.{}' disables CSRF protection for unsafe methods with session authentication. Restore CSRF or verify an independent request-authentication mechanism prevents cross-site actions.",
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
                    "Endpoint '{}.{}' (auth=\"{}\") calls .sudo(). Check authorization before each privileged operation and validate user-controlled record IDs.{}",
                    ctrl.class_name,
                    ctrl.name,
                    auth.unwrap_or("?"),
                    if ctrl.checks_token_access { " _document_check_access is present, but its coverage is not verified." } else { "" }
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
            ..Default::default()
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
    fn test_all_or_public_token_marks_global_acl_intentional() {
        // xml_id token: write demoted to log-only, read silenced.
        let mut rec = access_csv("", ["1", "1", "0", "0"]);
        rec.xml_id = "access_res_partner_all".to_string();
        let found = analyze_records(&[rec]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "acl-global-write");
        assert_eq!(found[0].severity, SEVERITY_WARNING);

        // Token anywhere, not just as suffix; "public" counts too.
        for xml_id in ["access_res_partner_all_x", "access_public_partner"] {
            let mut rec = access_csv("", ["1", "0", "0", "0"]);
            rec.xml_id = xml_id.to_string();
            assert!(analyze_records(&[rec]).is_empty(), "{xml_id}");
        }

        // Token match only: "install" contains "all" but must still warn.
        let mut rec = access_csv("", ["1", "0", "0", "0"]);
        rec.xml_id = "access_module_install".to_string();
        assert_eq!(analyze_records(&[rec]).len(), 1);

        // "name" field works too (CSV rows where the xml_id differs).
        let mut rec = access_csv("", ["1", "0", "0", "0"]);
        rec.fields.as_mut().unwrap()["name"] = serde_json::json!("res.partner all");
        assert_eq!(analyze_records(&[rec.clone()]).len(), 1); // " all" ≠ "_all"
        rec.fields.as_mut().unwrap()["name"] = serde_json::json!("access_res_partner_all");
        assert!(analyze_records(&[rec]).is_empty());
    }

    #[test]
    fn test_normal_group_acl_is_clean() {
        let found = analyze_records(&[access_csv("base.group_user", ["1", "1", "1", "1"])]);
        assert!(found.is_empty());
    }

    #[test]
    fn test_slash_id_csv_header_resolves_like_colon() {
        // addons/lunch's ir.model.access.csv uses "model_id/id" / "group_id/id"
        // instead of the ":id" suffix - a real group must not be mistaken for
        // "no group set".
        let rec = RecordAnalysisInfo {
            xml_id: "lunch_alert_access".to_string(),
            model: "ir.model.access".to_string(),
            noupdate: false,
            fields: Some(serde_json::json!({
                "name": "access_lunch_alert_user",
                "model_id/id": "model_lunch_alert",
                "group_id/id": "base.group_user",
                "perm_read": "1",
                "perm_write": "0",
                "perm_create": "0",
                "perm_unlink": "0",
            })),
            ..Default::default()
        };
        assert!(analyze_records(&[rec]).is_empty());
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
    fn test_portal_write_named_after_its_group_is_demoted() {
        // e.g. Odoo core's auth_passkey.access_auth_passkey_key_portal:
        // xml_id ends in "_portal" and grants write to base.group_portal -
        // a deliberate self-service ACL, not an oversight.
        let mut rec = access_csv("base.group_portal", ["1", "1", "0", "0"]);
        rec.xml_id = "access_auth_passkey_key_portal".to_string();
        let found = analyze_records(&[rec]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "acl-public-write");
        assert_eq!(found[0].severity, SEVERITY_WARNING);

        // Same naming convention via the "name" field instead of xml_id.
        let mut rec = access_csv("base.group_portal", ["1", "1", "0", "0"]);
        rec.fields.as_mut().unwrap()["name"] = serde_json::json!("access_auth_passkey_key_portal");
        let found = analyze_records(&[rec]);
        assert_eq!(found[0].severity, SEVERITY_WARNING);
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
            ..Default::default()
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
            ..Default::default()
        };
        assert!(analyze_records(&[rec_admin]).is_empty());
    }

    #[test]
    fn test_admin_group_without_module_prefix_is_not_escalation() {
        // e.g. Odoo core's own base/security/ir.model.access.csv:
        // access_ir_model_group_erp_manager grants write on model_ir_model
        // to "group_erp_manager", not "base.group_erp_manager" - same-module
        // xml_id refs drop the module prefix. Must still be recognized as
        // the admin group, not flagged as escalation.
        for group in ["group_erp_manager", "group_system"] {
            let rec = RecordAnalysisInfo {
                xml_id: "access_ir_model_group_erp_manager".to_string(),
                model: "ir.model.access".to_string(),
                noupdate: false,
                fields: Some(serde_json::json!({
                    "model_id:id": "model_ir_model",
                    "group_id:id": group,
                    "perm_read": "1",
                    "perm_write": "1",
                    "perm_create": "1",
                    "perm_unlink": "1",
                })),
                ..Default::default()
            };
            assert!(analyze_records(&[rec]).is_empty(), "group {group}");
        }
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
            ..Default::default()
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

        // Manager/admin group: canonical unlock pattern, not a finding.
        assert!(analyze_records(&[rule(
            "[]",
            Some("[(4, ref('sales_team.group_sale_manager'))]"),
        )])
        .is_empty());
        assert!(
            analyze_records(&[rule("[(1,'=',1)]", Some("[(4, ref('base.group_system'))]"),)])
                .is_empty()
        );

        // "_all" naming marks a deliberately global rule: not a finding.
        let mut all_rule = rule("[(1,'=',1)]", Some("[(4, ref('base.group_user'))]"));
        all_rule.xml_id = "rule_settlement_all".to_string();
        assert!(analyze_records(&[all_rule]).is_empty());

        // Plain internal group: minor (log-only). Also covers the `[]` domain form.
        let found = analyze_records(&[rule("[]", Some("[(4, ref('base.group_user'))]"))]);
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

    #[test]
    fn test_readonly_rule_needs_explicit_flags_not_just_a_name() {
        // Named "readonly" but doesn't actually turn off write/create/unlink
        // (Odoo defaults those to True): still a full bypass, still warns.
        let mut named_only = rule("[(1,'=',1)]", Some("[(4, ref('base.group_user'))]"));
        named_only.xml_id = "account_analytic_line_rule_readonly_user".to_string();
        let found = analyze_records(&[named_only]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "rule-group-bypass");

        // Explicit perm_write/create/unlink = False: genuinely read-only,
        // not a finding - the name is irrelevant, only the flags matter.
        let mut readonly = rule("[(1,'=',1)]", Some("[(4, ref('base.group_user'))]"));
        readonly.fields.as_mut().unwrap()["perm_write"] = serde_json::json!("False");
        readonly.fields.as_mut().unwrap()["perm_create"] = serde_json::json!("False");
        readonly.fields.as_mut().unwrap()["perm_unlink"] = serde_json::json!("False");
        assert!(analyze_records(&[readonly]).is_empty());

        // Only partially narrowed (perm_create still open): still a finding.
        let mut partial = rule("[(1,'=',1)]", Some("[(4, ref('base.group_user'))]"));
        partial.fields.as_mut().unwrap()["perm_write"] = serde_json::json!("False");
        partial.fields.as_mut().unwrap()["perm_unlink"] = serde_json::json!("False");
        assert_eq!(analyze_records(&[partial]).len(), 1);

        // Portal/public stays grave even when genuinely read-only: an
        // external group reading every record of the model is still a leak.
        let mut portal_readonly = rule("[(1,'=',1)]", Some("[(4, ref('base.group_portal'))]"));
        portal_readonly.fields.as_mut().unwrap()["perm_write"] = serde_json::json!("False");
        portal_readonly.fields.as_mut().unwrap()["perm_create"] = serde_json::json!("False");
        portal_readonly.fields.as_mut().unwrap()["perm_unlink"] = serde_json::json!("False");
        let found = analyze_records(&[portal_readonly]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "rule-public-bypass");
        assert_eq!(found[0].severity, SEVERITY_ERROR);
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
            ..Default::default()
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
    fn test_public_sudo_requires_review_even_with_a_token_check() {
        let found = analyze_controllers(&[route(Some("public"), None, &[], true)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "route-public-sudo");
        assert_eq!(found[0].severity, SEVERITY_WARNING);

        // auth="none" is inventory, not a finding on its own.
        let found = analyze_controllers(&[route(Some("none"), None, &[], true)]);
        let codes: Vec<&str> = found.iter().map(|w| w.code.as_str()).collect();
        assert_eq!(codes, vec!["route-public-sudo"]);
        assert!(analyze_controllers(&[route(Some("none"), None, &[], false)]).is_empty());

        // sudo behind an authenticated route: normal, clean.
        assert!(analyze_controllers(&[route(Some("user"), None, &[], true)]).is_empty());
        // A token check may protect another record or only one branch.
        let mut portal = route(Some("public"), None, &[], true);
        portal.checks_token_access = true;
        let found = analyze_controllers(&[portal]);
        assert_eq!(found.len(), 1);
        assert!(found[0].message.contains("coverage is not verified"));
        // Unknown auth (inherited-route override): no guessing, clean.
        assert!(analyze_controllers(&[route(None, None, &[], true)]).is_empty());
    }

    #[test]
    fn test_restricted_domains_inactive_rules_and_safe_methods_are_not_findings() {
        for domain in [
            "['&', (1, '=', 1), ('user_id', '=', user.id)]",
            "['!', (1, '=', 1)]",
            "[(1, '=', 1), ('company_id', '=', company_id)]",
        ] {
            assert!(
                analyze_records(&[rule(domain, Some("[(4, ref('base.group_portal'))]"))])
                    .is_empty()
            );
        }
        let mut inactive = rule("[]", Some("[(4, ref('base.group_portal'))]"));
        inactive.fields.as_mut().unwrap()["active"] = serde_json::json!("False");
        assert!(analyze_records(&[inactive]).is_empty());
        assert!(analyze_records(&[rule("[]", Some("[]"))]).is_empty());
        assert!(analyze_controllers(&[route(
            Some("user"),
            Some(false),
            &["GET", "HEAD", "OPTIONS"],
            false
        )])
        .is_empty());

        let mut acl = access_csv("custom.group_system", ["1", "1", "0", "0"]);
        acl.fields.as_mut().unwrap()["model_id:id"] = serde_json::json!("base.model_res_users");
        assert!(analyze_records(&[acl])
            .iter()
            .any(|w| w.code == "acl-privilege-escalation"));
    }
}
