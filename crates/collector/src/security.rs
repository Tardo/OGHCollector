// Copyright Alexandre D. Díaz
//! Static security checks over a module's analyzed records (ir.model.access
//! rows from CSV/XML and ir.rule records; Odoo 20's ir.access rows are
//! rewritten into those first, see `expand_ir_access`), HTTP controllers, code-level
//! sinks (SQL formatting, eval, pickle, shell, TLS) and Html fields. Findings
//! identify review priorities, not confirmed exploits. Effective access also
//! depends on other modules, global rules, field restrictions and Python code.
//! "error" (shown prominently on ommd.info) is kept for high-precision
//! rules; calibrate changes with analyzer.rs's ignored `calibrate` test.
//!
//! Static security checks over the analyzed records (lax `ir.model.access` write grants — minor on the
//! module's abstract models, its wizards that don't call sudo(), labels (`*.tag`), or when a rule
//! keeps that ACL's users to their own rows; group-less *read* grants
//! aren't reported, credentials are —, always-true portal/public
//! `ir.rule` domains correlated with the module's own ACLs (an internal group's is Odoo's "sees
//! all" pattern, not reported), privilege-escalation ACLs, portal/public
//! read of credentials — fields of a model the module opens to them, or added to `res.company`/
//! `website` —, users created with a fixed password), HTTP controllers (CSRF disabled: minor, it
//! needs a victim; public handlers acting under sudo() or on the session only aren't reported; mass
//! assignment of the raw request payload) and code-level facts (SQL injection: grave only when the
//! formatted value is caller-controlled — a parameter of an RPC-callable model method or route, the
//! request payload, or the context, followed through locals and the class's private helpers, also
//! when the SQL text is built away from its execute(), unless it went through the ORM's query
//! builders —, regex HTML tag filters that `.*`/`[^>]` let markup slip through, caller input picking
//! documents past the caller's rights — `sudo()`, `with_user()`, a route's elevated record env — in a
//! route or RPC method that checks no access, private methods run on a record whose model and id a
//! route's caller sends, and an RPC method writing
//! its caller's whole dict under `sudo()` (grave), eval/exec, pickle/unsafe YAML, `shell=True`,
//! `verify=False`, user-writable `Html(sanitize=False)`). `test_*`/"Tests"-category modules are
//! skipped, and so are records from the manifest's `demo` files. Every
//! finding is stored in `module_security_warning`; grave ones (`error`) are shown on the module
//! detail page, minor ones (`warning`) are counted there and also logged as a `system_event` the
//! first time they appear for a version snapshot. The analyzer also extracts every `http.route`
//! endpoint (paths, resolved auth, type, methods, csrf, sudo usage) into `module_controller`, shown
//! on the module page and served via `/api/v1/module/*` and MCP `get_module_code_analysis`.
use sqlitedb::models::module_code_analysis::{
    ControllerAnalysisInfo, MigrationFactInfo, ModelAnalysisInfo, ModuleAnalysisInfo,
    RecordAnalysisInfo,
};
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
    // Create alone can add records but not alter anyone else's (core utm
    // grants it to portal on purpose): grave only with write or unlink.
    let tampers = perm(rec, "perm_write") || perm(rec, "perm_unlink");

    match group {
        None => {
            let intentional = is_intentional_global(rec) || !tampers;
            if !write_perms.is_empty() {
                out.push(warning(
                    rec,
                    if intentional { SEVERITY_WARNING } else { SEVERITY_ERROR },
                    "acl-global-write",
                    format!(
                        "ACL grants {write_perms} on '{model_label}' to every user (portal/public included): no group is set. Verify record rules and test access as an unrelated public/portal user. CWE-732."
                    ),
                ));
            }
        }
        Some(g) if is_core_id(g, &PUBLIC_GROUP_XML_IDS) && !write_perms.is_empty() => {
            let intentional = !tampers
                || is_intentional_global(rec)
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
                    "ACL grants {write_perms} on '{model_label}' to '{g}'. Verify record rules restrict these operations to intended records, including records owned by other users/companies. CWE-732."
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
                        "ACL grants {write_perms} on security model '{model_ref}' to {}. Review field restrictions and Python guards for possible privilege escalation; the ACL alone does not prove exploitability. CWE-269.",
                        group.map(|g| format!("group '{g}'")).unwrap_or_else(|| "every user".to_string())
                    ),
                ));
            }
        }
    }
}

/// A user that (non-demo) data creates with a fixed password: every database
/// installing the module gets that login with a known password. base's own
/// `user_admin` is the exception (the database manager sets its password
/// when it creates the database); another module writing `base.user_admin`
/// resets the admin's password, and is reported. So is a user whose groups
/// the record empties (`[(5,)]`): a login with no rights, like 7.0's
/// portal_anonymous user.
fn check_user_password(rec: &RecordAnalysisInfo, out: &mut Vec<SecurityWarningInfo>) {
    let fixed = ["password", "new_password"].iter().any(|f| {
        field_str(rec, f).is_some_and(|v| {
            !v.starts_with("ref(") && !matches!(v, "False" | "None" | "''" | "\"\"")
        })
    });
    let no_groups = ["groups_id", "group_ids"].iter().any(|f| {
        field_str(rec, f).is_some_and(|v| {
            let v: String = v.chars().filter(|c| !c.is_whitespace()).collect();
            matches!(
                v.as_str(),
                "[(5,)]" | "[(6,0,[])]" | "[Command.set([])]" | "[Command.clear()]"
            )
        })
    });
    if !fixed || no_groups || rec.xml_id == "user_admin" || perm_explicit_false(rec, "active") {
        return;
    }
    let login = field_str(rec, "login").unwrap_or(&rec.xml_id);
    out.push(warning(
        rec,
        SEVERITY_ERROR,
        "data-user-password",
        format!(
            "Data (not demo) gives user '{login}' a fixed password: every database installing this module has that login with a password anyone can read here. Create it without a password (or inactive) and let an admin set one. CWE-798."
        ),
    ));
}

/// `model_x` / `module.model_x` / `ref('module.model_x')` -> `model_x`.
fn local_id(value: &str) -> &str {
    let v = strip_ref(value);
    v.rsplit('.').next().unwrap_or(v)
}

/// True when this module's own ACLs for (model, group) exist and all grant
/// nothing: a rule can only widen operations an ACL already allows, so an
/// always-true rule over a 0,0,0,0 ACL (e.g. core calendar's portal
/// attendee rule) opens nothing. ACLs from other modules can't be seen
/// here, hence "no ACL in this module" keeps the finding.
fn acl_denies_all(records: &[RecordAnalysisInfo], model: &str, group: &str) -> bool {
    let mut acls = records.iter().filter(|r| {
        r.model == "ir.model.access"
            && field_str(r, "model_id").is_some_and(|m| local_id(m) == local_id(model))
            && field_str(r, "group_id").is_some_and(|g| local_id(g) == local_id(group))
    });
    let mut any = false;
    let all_denied = acls.all(|a| {
        any = true;
        ["perm_read", "perm_write", "perm_create", "perm_unlink"]
            .iter()
            .all(|p| !perm(a, p))
    });
    any && all_denied
}

fn check_rule(
    rec: &RecordAnalysisInfo,
    records: &[RecordAnalysisInfo],
    out: &mut Vec<SecurityWarningInfo>,
) {
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
            let model = field_str(rec, "model_id");
            if groups
                .split(['\'', '"'])
                .filter(|g| is_core_id(g, &PUBLIC_GROUP_XML_IDS))
                .all(|g| model.is_some_and(|m| acl_denies_all(records, m, g)))
            {
                return;
            }
            out.push(warning(
                rec,
                SEVERITY_ERROR,
                "rule-public-bypass",
                "Always-true portal/public group rule removes restrictions from other group rules for applicable operations. ACLs and global rules still apply; test access to other users' records. CWE-863.".to_string(),
            ));
        }
        // An always-true rule for an internal group is how Odoo grants
        // "officer/manager sees all": not reported.
        _ => {}
    }
}

/// Odoo core and OCA ship `test_*` modules (category "Hidden/Tests") that
/// only exist for their own test suites and are never installed in
/// production; their deliberately loose ACLs used to be ~70% of all grave
/// findings. A `_test` suffix is not used: `account_test`/`payment_test`
/// are real, installable modules. OCA's `*_demo` modules (`base_rest_demo`,
/// `role_policy_demo`) are showcases of another module, skipped too.
pub fn is_test_module(technical_name: &str, category: &str) -> bool {
    technical_name.starts_with("test_")
        || technical_name.ends_with("_demo")
        || category
            .split('/')
            .any(|c| matches!(c.trim().to_lowercase().as_str(), "test" | "tests"))
}

/// Credential-like field names. Not credentials: `access_token`
/// (portal.mixin's per-record share link, already held by whoever reads the
/// record), hashes, and the browser keys (Google Maps, Google Fonts) the
/// website's own JS sends to Google.
fn is_secret_field_name(name: &str) -> bool {
    let t: Vec<&str> = name.split('_').collect();
    let pair = |a: &str, b: &str| t.windows(2).any(|w| w == [a, b]);
    !matches!(name, "google_maps_api_key" | "google_font_api_key")
        && !pair("access", "token")
        && !t.iter().any(|w| matches!(*w, "crypt" | "hash" | "hashed"))
        && (t
            .iter()
            .any(|w| matches!(*w, "secret" | "password" | "passwd" | "token" | "apikey"))
            || pair("api", "key")
            || pair("private", "key"))
}

/// A record rule of this module that limits `group` (None: a global rule,
/// so everyone) to the user's own records on `model` for operation `perm`:
/// each user then reads only their own secrets (core survey's answer
/// tokens), or edits only their own rows (core ir.ui.view.custom).
/// Multi-company domains (`user.company_id`, `company_ids`) don't count.
fn restricted_to_own(
    records: &[RecordAnalysisInfo],
    model: &str,
    group: Option<&str>,
    perm: &str,
) -> bool {
    records.iter().any(|r| {
        r.model == "ir.rule"
            && !perm_explicit_false(r, "active")
            && !perm_explicit_false(r, perm)
            && field_str(r, "model_id").is_some_and(|m| local_id(m) == model)
            && field_str(r, "groups").is_none_or(|gs| {
                matches!(gs, "[]" | "False" | "[(5,)]" | "[(6, 0, [])]")
                    || group.is_some_and(|group| {
                        gs.split(['\'', '"'])
                            .any(|g| local_id(g) == local_id(group))
                    })
            })
            && field_str(r, "domain_force").is_some_and(own_domain)
    })
}

/// A domain naming the current user (`uid`, `user.id`, `user.partner_id`).
fn own_domain(d: &str) -> bool {
    d.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .any(|t| {
            t == "uid"
                || matches!(
                    t.strip_prefix("user.").and_then(|r| r.split('.').next()),
                    Some("id" | "partner_id" | "commercial_partner_id")
                )
        })
}

/// The stored credential fields of one model class: none for wizards or
/// models that override `read()` (core res.users masks its password there);
/// computed/related fields and fields another field's inverse writes (OCA
/// keychain stores ciphertext that way) aren't stored credentials.
fn secret_fields(m: &ModelAnalysisInfo) -> Vec<String> {
    let kind = m
        .attrs
        .as_ref()
        .and_then(|a| a.get("kind"))
        .and_then(|k| k.as_str());
    if matches!(kind, Some("TransientModel" | "AbstractModel"))
        || m.methods.iter().any(|me| me.name == "read")
    {
        return Vec::new();
    }
    let has =
        |a: &Option<serde_json::Value>, k: &str| a.as_ref().is_some_and(|a| a.get(k).is_some());
    m.fields
        .iter()
        .filter(|f| {
            matches!(f.field_type.as_str(), "Char" | "Text")
                && is_secret_field_name(&f.name)
                && !["groups", "compute", "related"]
                    .iter()
                    .any(|k| has(&f.attrs, k))
                && !m.fields.iter().any(|o| {
                    o.name != f.name && o.name.contains(&f.name) && has(&o.attrs, "inverse")
                })
        })
        .map(|f| f.name.clone())
        .collect()
}

/// Core models every user can read (portal and public included) whatever
/// module adds fields to them: core's own access rights grant it, in every
/// version. l10n_ro_edi kept its eFactura secret on res.company until
/// Odoo's 095e7c996e.
const EVERYONE_READABLE_MODELS: [&str; 2] = ["res.company", "website"];

/// Credentials readable by portal/public users through `search_read`
/// (OCA mail_gateway's bot `token`, fixed in 0fd2edc): a read grant of this
/// module for portal/public (or for everyone) on a model whose fields hold
/// them with no `groups=`, or such fields added to an
/// `EVERYONE_READABLE_MODELS` model. Read-only grants are otherwise silent
/// (normal for portal), so this is the one place that looks at what the
/// readable model contains. Grants a rule narrows to the user's own records
/// don't count.
pub fn analyze_secret_reads(
    records: &[RecordAnalysisInfo],
    models: &[ModelAnalysisInfo],
) -> Vec<SecurityWarningInfo> {
    let mut out = Vec::new();
    let mut reported: Vec<&str> = Vec::new();
    let secrets_of = |name: &str| {
        let mut secrets: Vec<String> = models
            .iter()
            .filter(|m| m.model_name == name)
            .flat_map(secret_fields)
            .collect();
        secrets.sort();
        secrets.dedup();
        secrets
    };
    for rec in records {
        if rec.model != "ir.model.access"
            || perm_explicit_false(rec, "active")
            || !perm(rec, "perm_read")
        {
            continue;
        }
        let group = field_str(rec, "group_id").map(strip_ref);
        if group.is_some_and(|g| !is_core_id(g, &PUBLIC_GROUP_XML_IDS)) {
            continue;
        }
        let Some(model_ref) = field_str(rec, "model_id").map(local_id) else {
            continue;
        };
        if restricted_to_own(
            records,
            model_ref,
            Some(group.unwrap_or("base.group_portal")),
            "perm_read",
        ) {
            continue;
        }
        let Some(model) = models
            .iter()
            .find(|m| model_ref == format!("model_{}", m.model_name.replace('.', "_")))
        else {
            continue;
        };
        let secrets = secrets_of(&model.model_name);
        if secrets.is_empty() {
            continue;
        }
        reported.push(&model.model_name);
        let who = group.map_or("every user (portal/public included)".to_string(), |g| {
            format!("'{g}'")
        });
        out.push(warning(
            rec,
            SEVERITY_ERROR,
            "acl-public-read-secret",
            format!(
                "ACL grants read on '{}' to {who}, and its credential fields ({}) have no `groups=`: those users can read them over RPC. Restrict the fields with `groups=` or drop the grant. CWE-200.",
                model.model_name,
                secrets.join(", ")
            ),
        ));
    }
    for name in EVERYONE_READABLE_MODELS {
        let secrets = secrets_of(name);
        if secrets.is_empty() || reported.contains(&name) {
            continue;
        }
        out.push(SecurityWarningInfo {
            severity: SEVERITY_ERROR.to_string(),
            code: "acl-public-read-secret".to_string(),
            message: format!(
                "Adds credential fields ({}) with no `groups=` to '{name}', which every user (portal/public included) can read under core's access rights: they can read them over RPC. Restrict the fields with `groups=` (e.g. base.group_system). CWE-200.",
                secrets.join(", ")
            ),
            xml_id: Some(name.to_string()),
            file: None,
            line: None,
        });
    }
    out
}

/// Odoo 20 replaced `ir.model.access` + `ir.rule` with one `ir.access`
/// model (`model_id`, `group_id`, `operation` as a subset of "crud",
/// optional `domain`). A row with a group is a permission: an ACL for that
/// group, limited by its domain if any (a group rule then); a row without
/// one is a restriction, i.e. a global rule. `base.group_everyone` (every
/// user, portal and public included) is the old group-less ACL. Each row is
/// rewritten into those older records, under its own xml_id, so every
/// ACL/rule check above applies unchanged.
const SCOPED_DOMAIN: &str = "ir_access_domain";

fn expand_ir_access(records: Vec<RecordAnalysisInfo>) -> Vec<RecordAnalysisInfo> {
    let mut out = Vec::with_capacity(records.len());
    for rec in records {
        if rec.model != "ir.access" {
            out.push(rec);
            continue;
        }
        let ops = field_str(&rec, "operation").unwrap_or("");
        // CSV rows give the model's name (`mail.message`), XML a ref.
        let model = field_str(&rec, "model_id").map(|m| {
            let m = strip_ref(m);
            if m.contains("model_") {
                m.to_string()
            } else {
                format!("model_{}", m.replace('.', "_"))
            }
        });
        let group = field_str(&rec, "group_id")
            .map(strip_ref)
            .filter(|g| *g != "base.group_everyone")
            .map(str::to_string);
        let domain = field_str(&rec, "domain").map(str::to_string);
        let flag = |c: char| serde_json::json!(if ops.contains(c) { "1" } else { "0" });
        let perms = |mut fields: serde_json::Value| {
            fields["perm_read"] = flag('r');
            fields["perm_write"] = flag('u');
            fields["perm_create"] = flag('c');
            fields["perm_unlink"] = flag('d');
            fields
        };
        let derived = |model_name: &str, fields: serde_json::Value| RecordAnalysisInfo {
            model: model_name.to_string(),
            fields: Some(fields),
            ..rec.clone()
        };
        let has_group = field_str(&rec, "group_id").is_some();
        if has_group {
            let mut acl = serde_json::json!({ "model_id": model });
            if let Some(g) = &group {
                acl["group_id"] = serde_json::json!(g);
            }
            if let Some(d) = domain.as_deref().filter(|d| !is_permissive_domain(Some(d))) {
                // Not a field of ir.model.access: tells demote_scratch_and_own_grants
                // this grant only covers the records of its own domain.
                acl[SCOPED_DOMAIN] = serde_json::json!(d);
            }
            out.push(derived("ir.model.access", perms(acl)));
        }
        // Only a restriction becomes a rule: permissions OR together, so one
        // row's domain never narrows another's grant (it rides on its own
        // ACL as SCOPED_DOMAIN instead).
        if let Some(d) = domain.filter(|_| !has_group) {
            let rule = serde_json::json!({ "model_id": model, "domain_force": d });
            out.push(derived("ir.rule", perms(rule)));
        }
    }
    out
}

/// Every security finding for one analyzed module.
pub fn analyze_module(
    technical_name: &str,
    category: &str,
    analysis: &ModuleAnalysisInfo,
) -> Vec<SecurityWarningInfo> {
    if is_test_module(technical_name, category) {
        return Vec::new();
    }
    // Demo files never reach a production database.
    let records: Vec<RecordAnalysisInfo> = analysis
        .records
        .iter()
        .filter(|r| !r.demo)
        .cloned()
        .collect();
    let records = expand_ir_access(records);
    let secrets = analyze_secret_reads(&records, &analysis.models);
    let mut out = analyze_records(&records);
    demote_scratch_and_own_grants(&mut out, &records, &analysis.models);
    out.extend(secrets);
    out.extend(analyze_controllers(&analysis.controllers));
    out.extend(analyze_facts(&analysis.security_facts));
    out.extend(analyze_models(&analysis.models));
    out
}

/// Grave groupless/portal write grants that can't reach anyone's business
/// records become minor: on a wizard (TransientModel: per-request scratch
/// rows; the ORM still checks the records its buttons act on, unless they
/// use sudo()) or an AbstractModel (no table) of this module, on a model a
/// rule of this module narrows to the user's own rows for that ACL's group
/// (core ir.ui.view.custom, a global rule; website_crm_partner_assign's
/// portal leads, a portal rule), or on labels (`*.tag`). Not `*.category`
/// (product.category holds accounts and costing) nor `*.test`
/// (account_test's assertions hold code it runs).
fn demote_scratch_and_own_grants(
    out: &mut [SecurityWarningInfo],
    records: &[RecordAnalysisInfo],
    models: &[ModelAnalysisInfo],
) {
    let id = |m: &ModelAnalysisInfo| format!("model_{}", m.model_name.replace('.', "_"));
    // Any class of the model calling sudo() keeps its wizard grave
    // (sale_invoice_plan's wizard invoices the order in its context that way).
    let escalates: Vec<String> = models.iter().filter(|m| m.calls_sudo).map(id).collect();
    let scratch: Vec<String> = models
        .iter()
        .filter(|m| {
            match m
                .attrs
                .as_ref()
                .and_then(|a| a.get("kind"))
                .and_then(|k| k.as_str())
            {
                Some("AbstractModel") => true,
                Some("TransientModel") => !escalates.contains(&id(m)),
                _ => false,
            }
        })
        .map(id)
        .collect();
    for w in out.iter_mut().filter(|w| {
        w.severity == SEVERITY_ERROR
            && matches!(
                w.code.as_str(),
                "acl-global-write" | "acl-public-write" | "acl-privilege-escalation"
            )
    }) {
        let Some(acl) = records
            .iter()
            .find(|r| r.model == "ir.model.access" && w.xml_id.as_deref() == Some(&r.xml_id))
        else {
            continue;
        };
        // Odoo 20 ir.access: the grant carries its own domain. Any domain
        // bounds a write grant; escalation only stays grave unless it is the
        // user's own record (core's res.users self-write, whose fields are
        // guarded by SELF_WRITEABLE_FIELDS).
        if let Some(d) = field_str(acl, SCOPED_DOMAIN) {
            if w.code != "acl-privilege-escalation" || own_domain(d) {
                w.severity = SEVERITY_WARNING.to_string();
            }
            continue;
        }
        if w.code == "acl-privilege-escalation" {
            continue;
        }
        let group = field_str(acl, "group_id").map(strip_ref);
        let model = field_str(acl, "model_id").map(local_id);
        if model.is_some_and(|m| {
            scratch.iter().any(|s| s == m)
                || restricted_to_own(records, m, group, "perm_write")
                || matches!(m.rsplit('_').next(), Some("tag" | "tags"))
        }) {
            w.severity = SEVERITY_WARNING.to_string();
        }
    }
}

fn fact_warning(
    f: &MigrationFactInfo,
    severity: &str,
    code: &str,
    message: String,
) -> SecurityWarningInfo {
    SecurityWarningInfo {
        severity: severity.to_string(),
        code: code.to_string(),
        message,
        xml_id: f.context.clone(),
        file: f.file.clone(),
        line: f.line,
    }
}

/// Code-level sinks spotted by the analyzer (see `_analyze_sql_injection` /
/// `_analyze_dangerous_calls` in analyzer.rs). "error" is reserved for SQL
/// built from values an RPC/HTTP caller sets directly; everything else is a
/// review item, since static analysis can't tell where the value came from.
pub fn analyze_facts(facts: &[MigrationFactInfo]) -> Vec<SecurityWarningInfo> {
    let mut out = Vec::new();
    for f in facts {
        let snippet = f.detail.as_deref().unwrap_or("");
        let (severity, code, message) = match f.kind.as_str() {
            "sql_injection_tainted" => (
                SEVERITY_ERROR,
                "sql-injection",
                format!("SQL built by string formatting from caller-controlled input (an RPC-callable method's parameter, the request payload or the context): `{snippet}`. Pass values as query parameters (`cr.execute(query, params)`, `SQL(\"... %s\", value)`). CWE-89."),
            ),
            "sql_injection_admin" => (
                SEVERITY_WARNING,
                "sql-injection-review",
                format!("SQL built by string formatting from caller input: `{snippet}`. It runs after the ORM's write check on a core model only admins can write, so only an admin reaches it; still, pass values as query parameters (`cr.execute(query, params)`, `SQL()`). CWE-89."),
            ),
            "sql_injection" => (
                SEVERITY_WARNING,
                "sql-injection-review",
                format!("SQL built by string formatting: `{snippet}`. Values don't come straight from the caller here; verify none can be user-controlled or use query parameters / `SQL()`. CWE-89."),
            ),
            "code_eval" => (
                SEVERITY_WARNING,
                "code-eval",
                format!("`{snippet}` evaluates a non-literal expression with Python's eval/exec: use `ast.literal_eval` for data or `odoo.tools.safe_eval` for expressions. CWE-95."),
            ),
            "unsafe_deserialization" => (
                SEVERITY_WARNING,
                "unsafe-deserialization",
                format!("`{snippet}` deserializes with pickle/marshal/unsafe YAML, which executes code from crafted input: use JSON or `yaml.safe_load` for anything not fully trusted. CWE-502."),
            ),
            "shell_injection" => (
                SEVERITY_WARNING,
                "shell-injection",
                format!("`{snippet}` runs a shell command built at runtime: pass an argument list without `shell=True`. CWE-78."),
            ),
            "rpc_sudo_input" => (
                SEVERITY_WARNING,
                "rpc-sudo-input",
                format!("`{snippet}`: a public model method (callable over RPC by any logged-in user, portal included) hands its caller's input to sudo(), and checks no access itself: the caller picks records or values past access rights. Check access first (`check_access_rule`, `has_group`) or drop sudo(). CWE-639."),
            ),
            "rpc_sudo_write" => (
                SEVERITY_ERROR,
                "rpc-sudo-write",
                format!("`{snippet}`: a public model method (callable over RPC by any logged-in user, portal included) writes the values dict its caller sent under sudo(), and checks no access itself: the caller sets any field on records they couldn't write. Whitelist the fields and check access, or drop sudo(). CWE-269."),
            ),
            "route_sudo_input" => (
                SEVERITY_WARNING,
                "route-sudo-input",
                format!("`{snippet}`: an HTTP route hands request input to sudo() to pick records, and checks no access itself (`_document_check_access`, a token compared with `consteq`): whoever reaches the route can open other users' records. CWE-639."),
            ),
            "route_record_input" => (
                SEVERITY_WARNING,
                "route-record-input",
                format!("`{snippet}`: an HTTP route runs a private method on a record whose model and id the caller sends, and checks no access first: `browse()` checks nothing, so the caller acts on records they can't read. Fetch it through an access check (`_get_thread_with_access`, `check_access`) first. CWE-639."),
            ),
            "regex_html_filter" => (
                SEVERITY_WARNING,
                "html-regex-filter",
                format!("`{snippet}` filters HTML tags with a regular expression whose `.*` or `[^>]` runs across tag boundaries: nested or malformed markup (`<scr<script>ipt>`, `<a <b>>`) survives it. Parse the HTML (lxml, `odoo.tools.html_sanitize`) or keep `<`, `>` and newlines out of the repeated part. CWE-185."),
            ),
            "tls_verify_disabled" => (
                SEVERITY_WARNING,
                "tls-verify-disabled",
                format!("`{snippet}` disables TLS certificate verification, allowing man-in-the-middle on this connection. CWE-295."),
            ),
            _ => continue,
        };
        out.push(fact_warning(f, severity, code, message));
    }
    out
}

/// `fields.Html(sanitize=False)` stores raw HTML that any user able to write
/// the field can turn into stored XSS for everyone who views it.
pub fn analyze_models(models: &[ModelAnalysisInfo]) -> Vec<SecurityWarningInfo> {
    let mut out = Vec::new();
    for m in models {
        for f in &m.fields {
            let attr = |k: &str| {
                f.attrs
                    .as_ref()
                    .and_then(|a| a.get(k))
                    .and_then(|v| v.as_str())
            };
            // Computed (without inverse), related or group-restricted fields
            // aren't writable by arbitrary users: nothing to restrict further.
            let unsanitized = f.field_type == "Html"
                && attr("sanitize") == Some("False")
                && (attr("compute").is_none() || attr("inverse").is_some())
                && attr("related").is_none()
                && attr("groups").is_none();
            if unsanitized {
                out.push(SecurityWarningInfo {
                    severity: SEVERITY_WARNING.to_string(),
                    code: "html-unsanitized".to_string(),
                    message: format!("Html field '{}.{}' sets `sanitize=False`: content is stored and rendered unfiltered, so restrict who can write it. CWE-79.", m.model_name, f.name),
                    xml_id: Some(format!("{}.{}", m.model_name, f.name)),
                    file: None,
                    line: None,
                });
            }
        }
    }
    out
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
            "ir.rule" => check_rule(rec, records, &mut out),
            "res.users" => check_user_password(rec, &mut out),
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
                // Minor: exploiting it takes a logged-in victim visiting the
                // attacker's page. Odoo never CSRF-checks GET, so on a route
                // that also answers GET the flag removes no protection; and a
                // handler that can't reach the database only changes the session.
                let answers_get =
                    ctrl.methods.is_empty() || ctrl.methods.iter().any(|m| m == "GET");
                let why = if answers_get {
                    " It also answers GET, which Odoo never CSRF-checks: review it as a state change over GET."
                } else if ctrl.session_only {
                    " Its handler doesn't reach the database (session only)."
                } else {
                    " Restore CSRF or verify an independent request-authentication mechanism prevents cross-site actions."
                };
                out.push(controller_warning(
                    ctrl,
                    SEVERITY_WARNING,
                    "route-user-csrf-off",
                    format!(
                        "HTTP endpoint '{}.{}' disables CSRF protection for unsafe methods with session authentication.{why} CWE-352.",
                        ctrl.class_name, ctrl.name
                    ),
                ));
            } else if is_public && !ctrl.uses_sudo && !ctrl.session_only {
                // CSRF borrows the visitor's session: a handler acting under
                // sudo() (payment IPNs, webhooks) or only on the session does
                // the same whoever sends the request.
                out.push(controller_warning(
                    ctrl,
                    SEVERITY_WARNING,
                    "route-public-csrf-off",
                    format!(
                        "Public HTTP endpoint '{}.{}' disables CSRF protection and acts as the visitor (no sudo()): a logged-in visitor's session can be made to call it from another site. Fine for webhooks/callbacks, worth a review otherwise. CWE-352.",
                        ctrl.class_name, ctrl.name
                    ),
                ));
            }
        }
        if ctrl.mass_assignment {
            let grave = is_public || ctrl.uses_sudo;
            out.push(controller_warning(
                ctrl,
                if grave { SEVERITY_ERROR } else { SEVERITY_WARNING },
                "route-mass-assignment",
                format!(
                    "Endpoint '{}.{}' passes the raw request payload into write()/create(){}: the client can set any field of the model. Whitelist the accepted fields. CWE-915.",
                    ctrl.class_name,
                    ctrl.name,
                    if grave { " without access-rights checks (public route or sudo)" } else { "" }
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
    fn test_global_write_on_wizards_or_own_rows_is_minor() {
        let grave = |a: &ModuleAnalysisInfo| {
            analyze_module("m", "", a)
                .iter()
                .any(|w| w.code == "acl-global-write" && w.severity == SEVERITY_ERROR)
        };
        let mut a = ModuleAnalysisInfo {
            records: vec![access_csv("", ["1", "1", "1", "1"])],
            ..Default::default()
        };
        assert!(grave(&a));
        // A wizard (or AbstractModel) of this module: scratch rows.
        a.models = vec![ModelAnalysisInfo {
            model_name: "res.partner".to_string(),
            attrs: Some(serde_json::json!({"kind": "TransientModel"})),
            ..Default::default()
        }];
        assert!(!grave(&a));
        // ...unless any class of it escalates with sudo().
        a.models.push(ModelAnalysisInfo {
            model_name: "res.partner".to_string(),
            calls_sudo: true,
            ..Default::default()
        });
        assert!(grave(&a));
        // A global rule narrowing it to the user's own rows (core ir.ui.view.custom).
        a.models.clear();
        let mut own = rule("[('user_id','=',user.id)]", None);
        own.fields.as_mut().unwrap()["model_id"] = serde_json::json!("model_res_partner");
        a.records.push(own.clone());
        assert!(!grave(&a));
        // One binding portal only leaves internal users unrestricted.
        own.fields.as_mut().unwrap()["groups"] =
            serde_json::json!("[(4, ref('base.group_portal'))]");
        a.records[1] = own;
        assert!(grave(&a));
    }

    #[test]
    fn test_global_read_is_not_a_finding() {
        // Public catalogues (events, website menus, tags) are read this way;
        // credentials are acl-public-read-secret's job.
        assert!(analyze_records(&[access_csv("", ["1", "0", "0", "0"])]).is_empty());
    }

    #[test]
    fn test_global_write_on_labels_is_minor() {
        for (model, severity) in [
            ("model_blog_tag", SEVERITY_WARNING),
            ("model_res_partner_category_tags", SEVERITY_WARNING),
            ("model_product_category", SEVERITY_ERROR),
            ("model_accounting_assert_test", SEVERITY_ERROR),
        ] {
            let mut rec = access_csv("", ["1", "1", "1", "1"]);
            rec.fields.as_mut().unwrap()["model_id:id"] = serde_json::json!(model);
            let a = ModuleAnalysisInfo {
                records: vec![rec],
                ..Default::default()
            };
            let found = analyze_module("m", "", &a);
            assert_eq!(found[0].severity, severity, "{model}");
        }
    }

    // Odoo 20's ir.access rows (core addons/*/security/ir.access.csv).
    #[test]
    fn test_ir_access_rows_judged_like_acls_and_rules() {
        let row =
            |xml_id: &str, model: &str, group: &str, ops: &str, domain: &str| RecordAnalysisInfo {
                xml_id: xml_id.to_string(),
                model: "ir.access".to_string(),
                fields: Some(serde_json::json!({
                    "model_id": model,
                    "group_id/id": group,
                    "operation": ops,
                    "domain": domain,
                })),
                file: Some("security/ir.access.csv".to_string()),
                ..Default::default()
            };
        let a = ModuleAnalysisInfo {
            records: vec![
                row("thing_write", "x.thing", "base.group_portal", "ru", ""),
                // A scoped permission next to it doesn't narrow the open one.
                row(
                    "thing_write_own",
                    "x.thing",
                    "base.group_portal",
                    "u",
                    "[('user_id', '=', user.id)]",
                ),
                row(
                    "doc_write_scoped",
                    "x.doc",
                    "base.group_portal",
                    "ru",
                    "[('res_access_write', '=', True)]",
                ),
                row(
                    "everyone_write",
                    "x.other",
                    "base.group_everyone",
                    "cru",
                    "",
                ),
                row(
                    "self_write",
                    "res.users",
                    "base.group_user",
                    "u",
                    "[('id', '=', user.id)]",
                ),
                row("users_write", "res.groups", "base.group_user", "u", ""),
                row(
                    "restriction",
                    "x.thing",
                    "",
                    "crud",
                    "[('company_id', 'in', company_ids)]",
                ),
                row("portal_read", "x.thing", "base.group_portal", "r", ""),
            ],
            ..Default::default()
        };
        let mut got: Vec<(String, String, String)> = analyze_module("m", "", &a)
            .into_iter()
            .map(|w| (w.xml_id.unwrap_or_default(), w.code, w.severity))
            .collect();
        got.sort();
        // The portal grant's xml_id avoids "portal" (an intent token).
        let want = [
            ("doc_write_scoped", "acl-public-write", SEVERITY_WARNING),
            ("everyone_write", "acl-global-write", SEVERITY_ERROR),
            ("self_write", "acl-privilege-escalation", SEVERITY_WARNING),
            ("thing_write", "acl-public-write", SEVERITY_ERROR),
            ("thing_write_own", "acl-public-write", SEVERITY_WARNING),
            ("users_write", "acl-privilege-escalation", SEVERITY_ERROR),
        ];
        assert_eq!(
            got,
            want.iter()
                .map(|(x, c, s)| (x.to_string(), c.to_string(), s.to_string()))
                .collect::<Vec<_>>()
        );
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
        let severity = |xml_id: &str, name: Option<&str>| {
            let mut rec = access_csv("", ["1", "1", "0", "0"]);
            rec.xml_id = xml_id.to_string();
            if let Some(name) = name {
                rec.fields.as_mut().unwrap()["name"] = serde_json::json!(name);
            }
            analyze_records(&[rec])[0].severity.clone()
        };
        for xml_id in ["access_res_partner_all_x", "access_public_partner"] {
            assert_eq!(severity(xml_id, None), SEVERITY_WARNING, "{xml_id}");
        }
        // Token match only: "install" contains "all" but stays grave.
        assert_eq!(severity("access_module_install", None), SEVERITY_ERROR);
        // "name" field works too (CSV rows where the xml_id differs).
        assert_eq!(severity("x", Some("res.partner all")), SEVERITY_ERROR); // " all" ≠ "_all"
        assert_eq!(
            severity("x", Some("access_res_partner_all")),
            SEVERITY_WARNING
        );
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
        // Create-only (core utm's portal ACLs): reported, but not grave.
        let found = analyze_records(&[access_csv("base.group_portal", ["1", "0", "1", "0"])]);
        assert_eq!(found[0].severity, SEVERITY_WARNING);
        let found = analyze_records(&[access_csv("", ["1", "0", "1", "0"])]);
        assert_eq!(found[0].code, "acl-global-write");
        assert_eq!(found[0].severity, SEVERITY_WARNING);
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

        // Plain internal group: Odoo's "sees all" pattern, not a finding.
        assert!(analyze_records(&[rule("[]", Some("[(4, ref('base.group_user'))]"))]).is_empty());

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
        // (Odoo defaults those to True): still a full portal bypass.
        let mut named_only = rule("[(1,'=',1)]", Some("[(4, ref('base.group_portal'))]"));
        named_only.xml_id = "portal_rule_readonly".to_string();
        assert_eq!(analyze_records(&[named_only]).len(), 1);

        // Every perm explicitly off: the rule grants nothing.
        let mut off = rule("[(1,'=',1)]", Some("[(4, ref('base.group_portal'))]"));
        for p in ["perm_read", "perm_write", "perm_create", "perm_unlink"] {
            off.fields.as_mut().unwrap()[p] = serde_json::json!("False");
        }
        assert!(analyze_records(&[off]).is_empty());

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
    fn test_csrf_off_is_minor_and_public_sudo_is_silent() {
        let found = analyze_controllers(&[route(Some("user"), Some(false), &["POST"], false)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "route-user-csrf-off");
        assert_eq!(found[0].severity, SEVERITY_WARNING);
        assert_eq!(found[0].xml_id.as_deref(), Some("/demo/endpoint"));

        let found = analyze_controllers(&[route(Some("public"), Some(false), &["POST"], false)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].code, "route-public-csrf-off");
        assert_eq!(found[0].severity, SEVERITY_WARNING);
        // A public handler acting under sudo() (payment IPN, webhook) does the
        // same whoever sends the request: CSRF adds nothing.
        assert!(
            analyze_controllers(&[route(Some("public"), Some(false), &["POST"], true)]).is_empty()
        );

        // Also answers GET (no `methods`, or GET listed), or a session-only
        // handler: the flag opens nothing a GET wouldn't, minor.
        for methods in [&[][..], &["GET", "POST"][..]] {
            let found = analyze_controllers(&[route(Some("user"), Some(false), methods, false)]);
            assert_eq!(found[0].severity, SEVERITY_WARNING, "{methods:?}");
        }
        let mut beacon = route(Some("user"), Some(false), &["POST"], false);
        beacon.session_only = true;
        assert_eq!(analyze_controllers(&[beacon])[0].severity, SEVERITY_WARNING);

        // GET-only route: csrf=False is irrelevant, no finding.
        assert!(
            analyze_controllers(&[route(Some("user"), Some(false), &["GET"], false)]).is_empty()
        );
        // csrf untouched (framework default): clean.
        assert!(analyze_controllers(&[route(Some("user"), None, &["POST"], false)]).is_empty());
    }

    #[test]
    fn test_sudo_in_a_route_alone_is_not_a_finding() {
        // Only request input reaching sudo() is (`route_sudo_input`, a fact
        // from the extractor): public routes read config with sudo everywhere.
        for auth in [Some("public"), Some("none"), Some("user"), None] {
            assert!(analyze_controllers(&[route(auth, None, &[], true)]).is_empty());
        }
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

    #[test]
    fn test_test_modules_are_skipped_but_real_ones_are_not() {
        let analysis = ModuleAnalysisInfo {
            records: vec![access_csv("", ["1", "1", "0", "0"])],
            ..Default::default()
        };
        for (name, category) in [
            ("test_mail", "Hidden"),
            ("l10n_x_edi_tests", "Hidden/Tests"),
            ("profiler", "Tests"),
            ("role_policy_demo", "Tools"),
        ] {
            assert!(
                analyze_module(name, category, &analysis).is_empty(),
                "{name}"
            );
        }
        // Real, installable modules despite the name.
        for name in ["account_test", "payment_test"] {
            assert_eq!(
                analyze_module(name, "Accounting", &analysis).len(),
                1,
                "{name}"
            );
        }
    }

    #[test]
    fn test_always_true_portal_rule_over_a_deny_all_acl_is_not_a_finding() {
        // Odoo core calendar: always-true portal rule on calendar.attendee,
        // but the same module's portal ACL for it is 0,0,0,0.
        let mut rule_rec = rule("[(1, '=', 1)]", Some("[(4, ref('base.group_portal'))]"));
        rule_rec.fields.as_mut().unwrap()["model_id"] =
            serde_json::json!("ref('model_calendar_attendee')");
        let mut acl = access_csv("base.group_portal", ["0", "0", "0", "0"]);
        acl.fields.as_mut().unwrap()["model_id:id"] = serde_json::json!("model_calendar_attendee");
        assert!(analyze_records(&[rule_rec.clone(), acl.clone()]).is_empty());
        // The ACL grants read: every attendee becomes readable, still grave.
        acl.fields.as_mut().unwrap()["perm_read"] = serde_json::json!("1");
        let found = analyze_records(&[rule_rec.clone(), acl]);
        assert_eq!(found[0].code, "rule-public-bypass");
        // No ACL in this module (another module may grant it): kept.
        assert_eq!(analyze_records(&[rule_rec]).len(), 1);
    }

    #[test]
    fn test_code_facts_severity() {
        let fact = |kind: &str| MigrationFactInfo {
            kind: kind.to_string(),
            context: Some("X.method".to_string()),
            detail: Some("cr.execute(q % x)".to_string()),
            file: Some("models/x.py".to_string()),
            line: Some(7),
        };
        let found = analyze_facts(&[
            fact("sql_injection_tainted"),
            fact("sql_injection"),
            fact("sql_injection_admin"),
            fact("code_eval"),
            fact("unsafe_deserialization"),
            fact("shell_injection"),
            fact("tls_verify_disabled"),
            fact("rpc_sudo_write"),
            fact("rpc_sudo_input"),
            fact("route_sudo_input"),
            fact("route_record_input"),
            fact("unknown"),
        ]);
        let got: Vec<(&str, &str)> = found
            .iter()
            .map(|w| (w.code.as_str(), w.severity.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("sql-injection", SEVERITY_ERROR),
                ("sql-injection-review", SEVERITY_WARNING),
                ("sql-injection-review", SEVERITY_WARNING),
                ("code-eval", SEVERITY_WARNING),
                ("unsafe-deserialization", SEVERITY_WARNING),
                ("shell-injection", SEVERITY_WARNING),
                ("tls-verify-disabled", SEVERITY_WARNING),
                ("rpc-sudo-write", SEVERITY_ERROR),
                ("rpc-sudo-input", SEVERITY_WARNING),
                ("route-sudo-input", SEVERITY_WARNING),
                ("route-record-input", SEVERITY_WARNING),
            ]
        );
        assert_eq!(found[0].xml_id.as_deref(), Some("X.method"));
        assert_eq!(found[0].line, Some(7));
        assert!(found[0].message.contains("CWE-89"));
    }

    #[test]
    fn test_mass_assignment_is_grave_without_access_checks() {
        let mut public = route(Some("public"), None, &["POST"], false);
        public.mass_assignment = true;
        let found = analyze_controllers(&[public]);
        assert_eq!(found[0].code, "route-mass-assignment");
        assert_eq!(found[0].severity, SEVERITY_ERROR);
        let mut user = route(Some("user"), None, &["POST"], false);
        user.mass_assignment = true;
        assert_eq!(analyze_controllers(&[user])[0].severity, SEVERITY_WARNING);
    }

    #[test]
    fn test_unsanitized_html_only_when_user_writable() {
        let field = |attrs: serde_json::Value| ModelAnalysisInfo {
            model_name: "x.page".to_string(),
            fields: vec![sqlitedb::models::module_code_analysis::FieldAnalysisInfo {
                name: "body".to_string(),
                field_type: "Html".to_string(),
                relation: None,
                attrs: Some(attrs),
            }],
            ..Default::default()
        };
        assert_eq!(
            analyze_models(&[field(serde_json::json!({"sanitize": "False"}))]).len(),
            1
        );
        for attrs in [
            serde_json::json!({"sanitize": "False", "compute": "'_c'"}),
            serde_json::json!({"sanitize": "False", "groups": "'base.group_system'"}),
            serde_json::json!({"sanitize": "True"}),
        ] {
            assert!(analyze_models(&[field(attrs)]).is_empty());
        }
    }

    #[test]
    fn test_fixed_password_users_only_in_non_demo_data() {
        let user = |xml_id: &str, fields: serde_json::Value| RecordAnalysisInfo {
            xml_id: xml_id.to_string(),
            model: "res.users".to_string(),
            fields: Some(fields),
            ..Default::default()
        };
        let sentinel = user(
            "user_sentinel",
            serde_json::json!({"login": "sentinel", "password": "sentinel"}),
        );
        let found = analyze_records(std::slice::from_ref(&sentinel));
        assert_eq!(found[0].code, "data-user-password");
        assert_eq!(found[0].severity, SEVERITY_ERROR);
        assert!(!found[0].message.contains("password 'sentinel'"));
        // Another module resetting the admin's password is reported too.
        let reset = user("base.user_admin", serde_json::json!({"password": "x"}));
        assert_eq!(analyze_records(&[reset]).len(), 1);
        for quiet in [
            user(
                "user_admin",
                serde_json::json!({"login": "admin", "password": "admin"}),
            ),
            user("public_user", serde_json::json!({"login": "public"})),
            user("u", serde_json::json!({"password": "x", "active": "False"})),
            user("u", serde_json::json!({"new_password": "False"})),
            user(
                "anonymous_user",
                serde_json::json!({"password": "anonymous", "groups_id": "[(5,)]"}),
            ),
        ] {
            assert!(analyze_records(&[quiet]).is_empty());
        }
        // Demo files never reach production: analyze_module drops them.
        let analysis = ModuleAnalysisInfo {
            records: vec![RecordAnalysisInfo {
                demo: true,
                ..sentinel
            }],
            ..Default::default()
        };
        assert!(analyze_module("m", "Sales", &analysis).is_empty());
    }

    #[test]
    fn test_portal_read_of_stored_credentials_is_grave() {
        // OCA mail_gateway before 0fd2edc: portal could read the bot token.
        let model = |name: &str, attrs: serde_json::Value| ModelAnalysisInfo {
            model_name: "res.partner".to_string(),
            fields: vec![sqlitedb::models::module_code_analysis::FieldAnalysisInfo {
                name: name.to_string(),
                field_type: "Char".to_string(),
                relation: None,
                attrs: Some(attrs),
            }],
            ..Default::default()
        };
        let portal = access_csv("base.group_portal", ["1", "0", "0", "0"]);
        let found = analyze_secret_reads(
            std::slice::from_ref(&portal),
            &[model("token", serde_json::json!({}))],
        );
        assert_eq!(found[0].code, "acl-public-read-secret");
        assert_eq!(found[0].severity, SEVERITY_ERROR);
        // Not stored credentials, or restricted per field.
        for (name, attrs) in [
            ("access_token", serde_json::json!({})),
            ("password_crypt", serde_json::json!({})),
            (
                "token",
                serde_json::json!({"groups": "'base.group_system'"}),
            ),
            ("token", serde_json::json!({"compute": "'_c'"})),
        ] {
            let m = model(name, attrs);
            assert!(analyze_secret_reads(std::slice::from_ref(&portal), &[m]).is_empty());
        }
        {
            // OCA keychain: the stored password is clear_password's encrypted form.
            let mut m = model("password", serde_json::json!({}));
            let mut clear = m.fields[0].clone();
            clear.name = "clear_password".to_string();
            clear.attrs = Some(
                serde_json::json!({"compute": "'_compute_password'", "inverse": "'_inverse_set_password'"}),
            );
            m.fields.push(clear);
            assert!(analyze_secret_reads(std::slice::from_ref(&portal), &[m]).is_empty());
        }
        let token = [model("token", serde_json::json!({}))];
        // Internal users only.
        let internal = access_csv("base.group_user", ["1", "0", "0", "0"]);
        assert!(analyze_secret_reads(&[internal], &token).is_empty());
        // A rule narrows portal to its own records; a multi-company one doesn't.
        let mut own = rule(
            "[('create_uid', '=', user.id)]",
            Some("[(4, ref('base.group_portal'))]"),
        );
        own.fields.as_mut().unwrap()["model_id"] = serde_json::json!("ref('model_res_partner')");
        assert!(analyze_secret_reads(&[portal.clone(), own], &token).is_empty());
        let mut company = rule("[('company_id', 'child_of', [user.company_id.id])]", None);
        company.fields.as_mut().unwrap()["model_id"] =
            serde_json::json!("ref('model_res_partner')");
        assert_eq!(analyze_secret_reads(&[portal, company], &token).len(), 1);
        // No grant of its own: res.company is readable by everyone in core.
        let mut on_company = model("l10n_ro_edi_client_secret", serde_json::json!({}));
        on_company.model_name = "res.company".to_string();
        let found = analyze_secret_reads(&[], &[on_company]);
        assert_eq!(found[0].xml_id.as_deref(), Some("res.company"));
        assert_eq!(found[0].severity, SEVERITY_ERROR);
    }
}
