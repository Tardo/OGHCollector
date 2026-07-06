// Copyright Alexandre D. Díaz
use duct::cmd;
use fs_extra::dir::get_size;
use pyo3::prelude::*;
use pyo3::types::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;

use oghutils::version::OdooVersion;
use sqlitedb::models::module::{CommitterActivity, ManifestInfo};
use sqlitedb::models::module_code_analysis::ModuleAnalysisInfo;

use crate::gitclient::RepoInfo;

// Embedded Python analysis script: walks a module folder and, without
// executing any of its code, extracts the model classes it defines/extends
// (with their public methods - signature, decorators, docstring - and
// `fields.X(...)` assignments, including all keyword args), the
// ir.ui.view/template records its XML files declare, and every other record
// the module touches (security groups, record rules, cron jobs, access
// rights from ir.model.access.csv, ...) with its resolved noupdate flag -
// so a caller can tell "does this add a new access group" or "does this
// write demo data that only loads once" without re-parsing XML itself. Uses
// only the stdlib (ast, csv, xml.etree, os, json) so it runs with any Python
// available at build time - no odoo import required, and no ast.unparse
// (keeps it working on Python 3.8, which lacks it). Returns a single JSON
// string.
//
// The extra detail here (signatures, docstrings, decorator arguments, every
// field/model keyword argument) is deliberately captured beyond what the UI
// currently renders: the goal is for this data to be useful to an LLM (and
// eventually served over MCP), not just for the module detail page.
const ANALYZER_PY_SRC: &str = r#"
import ast
import csv
import json
import os
import xml.etree.ElementTree as ET

SKIP_DIRS = {"static", "i18n", "tests", "test", "__pycache__", ".git", "migrations"}
ACCESS_CSV_FILENAME = "ir.model.access.csv"
MODEL_BASE_ATTRS = {"Model", "TransientModel", "AbstractModel"}
RELATIONAL_FIELD_TYPES = {"Many2one", "One2many", "Many2many"}
DOCSTRING_LIMIT = 4000


def _truncate(s):
    if s is None:
        return None
    return s if len(s) <= DOCSTRING_LIMIT else s[:DOCSTRING_LIMIT] + "..."


def _expr_repr(node):
    # Reconstructs a best-effort, human/LLM-readable source representation of
    # a literal-ish expression (decorator args, field kwargs, arg defaults/
    # annotations) without relying on ast.unparse (Python 3.9+ only).
    if node is None:
        return None
    if isinstance(node, ast.Constant):
        return repr(node.value)
    if isinstance(node, (ast.List, ast.Tuple, ast.Set)):
        opening, closing = {"List": ("[", "]"), "Tuple": ("(", ")"), "Set": ("{", "}")}[
            type(node).__name__
        ]
        items = [x for x in (_expr_repr(el) for el in node.elts) if x is not None]
        return f"{opening}{', '.join(items)}{closing}"
    if isinstance(node, ast.Dict):
        pairs = []
        for k, v in zip(node.keys, node.values):
            k_repr = _expr_repr(k) if k is not None else "**"
            v_repr = _expr_repr(v)
            if v_repr is not None:
                pairs.append(f"{k_repr}: {v_repr}")
        return "{" + ", ".join(pairs) + "}"
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        base = _expr_repr(node.value)
        return f"{base}.{node.attr}" if base else node.attr
    if isinstance(node, ast.UnaryOp) and isinstance(node.op, ast.USub):
        inner = _expr_repr(node.operand)
        return f"-{inner}" if inner is not None else None
    if isinstance(node, ast.Starred):
        inner = _expr_repr(node.value)
        return f"*{inner}" if inner is not None else "*"
    if isinstance(node, ast.Call):
        func = _expr_repr(node.func) or "?"
        args = [x for x in (_expr_repr(a) for a in node.args) if x is not None]
        kwargs = [
            f"{kw.arg}={_expr_repr(kw.value)}" for kw in node.keywords if kw.arg is not None
        ]
        return f"{func}({', '.join(args + kwargs)})"
    return "<expr>"


def _is_model_class(node):
    for base in node.bases:
        if isinstance(base, ast.Attribute) and base.attr in MODEL_BASE_ATTRS:
            return True
        if isinstance(base, ast.Name) and base.id in MODEL_BASE_ATTRS:
            return True
    return False


def _const_str_list(value):
    if isinstance(value, ast.Constant) and isinstance(value.value, str):
        return [value.value]
    if isinstance(value, (ast.List, ast.Tuple)):
        out = []
        for el in value.elts:
            if isinstance(el, ast.Constant) and isinstance(el.value, str):
                out.append(el.value)
        return out
    return []


def _field_info(call):
    # Only `fields.Char(...)`, `fields.Many2one(...)`, etc. - not just any
    # `foo.bar()` call assigned at class level.
    if not isinstance(call.func, ast.Attribute):
        return None
    if not isinstance(call.func.value, ast.Name) or call.func.value.id != "fields":
        return None
    field_type = call.func.attr
    relation = None
    if field_type in RELATIONAL_FIELD_TYPES and call.args:
        first = call.args[0]
        if isinstance(first, ast.Constant) and isinstance(first.value, str):
            relation = first.value
    # Every keyword arg, verbatim (string/help/required/readonly/store/
    # compute/related/default/selection/size/digits/tracking/...) - Odoo field
    # kwargs aren't a fixed set, so capture them all rather than a curated few.
    attrs = {}
    # Positional args too (e.g. a Selection's option list, or a Char's label,
    # both commonly passed positionally rather than as a kwarg).
    positional = [x for x in (_expr_repr(a) for a in call.args) if x is not None]
    if positional:
        attrs["args"] = positional
    for kw in call.keywords:
        if kw.arg is None:
            continue
        if kw.arg == "comodel_name" and isinstance(kw.value, ast.Constant):
            relation = kw.value.value
        value_repr = _expr_repr(kw.value)
        if value_repr is not None:
            attrs[kw.arg] = value_repr
    return field_type, relation, (attrs or None)


def _arg_str(a, default=None):
    s = a.arg
    if a.annotation is not None:
        ann = _expr_repr(a.annotation)
        if ann:
            s += f": {ann}"
    if default is not None:
        d = _expr_repr(default)
        s += f"={d}" if d is not None else "=..."
    return s


def _signature(func):
    args = func.args
    parts = []
    posonly = list(args.posonlyargs)
    positional = posonly + list(args.args)
    defaults = list(args.defaults)
    num_no_default = len(positional) - len(defaults)
    for i, a in enumerate(positional):
        default = defaults[i - num_no_default] if i >= num_no_default else None
        parts.append(_arg_str(a, default))
        if posonly and i == len(posonly) - 1:
            parts.append("/")
    if args.vararg:
        parts.append("*" + _arg_str(args.vararg))
    elif args.kwonlyargs:
        parts.append("*")
    for a, d in zip(args.kwonlyargs, args.kw_defaults):
        parts.append(_arg_str(a, d))
    if args.kwarg:
        parts.append("**" + _arg_str(args.kwarg))
    signature = "(" + ", ".join(parts) + ")"
    if func.returns is not None:
        ret = _expr_repr(func.returns)
        if ret:
            signature += f" -> {ret}"
    return signature


def _analyze_python_source(source):
    out = []
    try:
        tree = ast.parse(source)
    except (SyntaxError, ValueError):
        return out
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef) or not _is_model_class(node):
            continue
        model_name = None
        inherit_names = []
        model_attrs = {}
        fields = []
        methods = []
        for base in node.bases:
            name = (
                base.attr
                if isinstance(base, ast.Attribute)
                else (base.id if isinstance(base, ast.Name) else None)
            )
            if name in MODEL_BASE_ATTRS:
                model_attrs["kind"] = name
                break
        for stmt in node.body:
            if (
                isinstance(stmt, ast.Assign)
                and len(stmt.targets) == 1
                and isinstance(stmt.targets[0], ast.Name)
            ):
                target_name = stmt.targets[0].id
                if target_name == "_name":
                    values = _const_str_list(stmt.value)
                    if values:
                        model_name = values[0]
                elif target_name == "_inherit":
                    inherit_names = _const_str_list(stmt.value)
                elif target_name in ("_description", "_rec_name", "_order", "_table"):
                    values = _const_str_list(stmt.value)
                    if values:
                        model_attrs[target_name.lstrip("_")] = values[0]
                elif target_name == "_inherits" and isinstance(stmt.value, ast.Dict):
                    delegation = {}
                    for k, v in zip(stmt.value.keys, stmt.value.values):
                        if isinstance(k, ast.Constant) and isinstance(v, ast.Constant):
                            delegation[k.value] = v.value
                    if delegation:
                        model_attrs["inherits_delegation"] = delegation
                elif isinstance(stmt.value, ast.Call):
                    info = _field_info(stmt.value)
                    if info:
                        field_type, relation, attrs = info
                        fields.append(
                            {
                                "name": target_name,
                                "field_type": field_type,
                                "relation": relation,
                                "attrs": attrs,
                            }
                        )
            elif isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                if stmt.name.startswith("_"):
                    continue
                decorators = [
                    x for x in (_expr_repr(dec) for dec in stmt.decorator_list) if x
                ]
                methods.append(
                    {
                        "name": stmt.name,
                        "decorators": decorators,
                        "signature": _signature(stmt),
                        "docstring": _truncate(ast.get_docstring(stmt, clean=True)),
                    }
                )
        effective_model = model_name or (inherit_names[0] if inherit_names else None)
        if not effective_model:
            continue
        is_new_model = model_name is not None and model_name not in inherit_names
        out.append(
            {
                "class_name": node.name,
                "model_name": effective_model,
                "inherit_from": inherit_names,
                "is_new_model": is_new_model,
                "docstring": _truncate(ast.get_docstring(node, clean=True)),
                "attrs": model_attrs or None,
                "fields": fields,
                "methods": methods,
            }
        )
    return out


def _arch_root_tag(field_el):
    # First child element inside <field name="arch" type="xml">, e.g. "form",
    # "tree", "search", "kanban" - tells an LLM what kind of view this is
    # without needing to load/parse the whole arch.
    for child in field_el:
        return child.tag
    return None


def _analyze_xml_source(data):
    out = []
    try:
        root = ET.fromstring(data)
    except ET.ParseError:
        return out
    for record in root.iter("record"):
        if record.attrib.get("model") != "ir.ui.view":
            continue
        xml_id = record.attrib.get("id")
        if not xml_id:
            continue
        name = None
        model = None
        inherit_xml_id = None
        view_type = None
        for field in record.findall("field"):
            fname = field.attrib.get("name")
            if fname == "name":
                name = (field.text or "").strip() or None
            elif fname == "model":
                model = (field.text or "").strip() or None
            elif fname == "inherit_id":
                inherit_xml_id = field.attrib.get("ref")
            elif fname == "arch":
                view_type = _arch_root_tag(field)
        out.append(
            {
                "xml_id": xml_id,
                "name": name,
                "model": model,
                "inherit_xml_id": inherit_xml_id,
                "view_type": view_type,
            }
        )
    for template in root.iter("template"):
        xml_id = template.attrib.get("id")
        if not xml_id:
            continue
        out.append(
            {
                "xml_id": xml_id,
                "name": template.attrib.get("name") or xml_id,
                "model": "ir.ui.view",
                "inherit_xml_id": template.attrib.get("inherit_id"),
                "view_type": "qweb",
            }
        )
    return out


def _field_value_repr(field_el):
    # A `<field>` inside a non-view record: a `ref`/`eval` attribute or its
    # text content, whichever is set - reused for both res.groups/ir.rule/
    # ir.cron-style XML records and CSV access-right rows below. Truncated
    # like docstrings since some fields (ir.actions.server code, qweb/
    # mail.template bodies) can be arbitrarily large.
    ref = field_el.attrib.get("ref")
    if ref:
        return f"ref({ref!r})"
    eval_attr = field_el.attrib.get("eval")
    if eval_attr is not None:
        eval_attr = eval_attr.strip()
        return _truncate(eval_attr) if eval_attr else None
    text = (field_el.text or "").strip()
    return _truncate(text) if text else None


def _analyze_xml_records(data):
    # Every non-view `<record>` a module defines: security groups
    # (res.groups), record rules (ir.rule), cron jobs (ir.cron), server
    # actions, demo/reference data, etc. - one generic mechanism instead of a
    # model-specific case for each, so "did this module add an access group"
    # is just "any record with model == res.groups" for the caller. noupdate
    # is resolved from the nearest ancestor `<data>`/`<odoo>` that sets it
    # (Odoo default: False), with per-record overrides honored.
    out = []
    try:
        root = ET.fromstring(data)
    except ET.ParseError:
        return out

    def walk(elem, noupdate):
        raw = elem.attrib.get("noupdate")
        current = raw.strip().lower() in ("1", "true") if raw is not None else noupdate
        if elem.tag == "record":
            model = elem.attrib.get("model")
            xml_id = elem.attrib.get("id")
            if model and xml_id and model != "ir.ui.view":
                fields = {}
                for field in elem.findall("field"):
                    fname = field.attrib.get("name")
                    if not fname:
                        continue
                    value = _field_value_repr(field)
                    if value is not None:
                        fields[fname] = value
                out.append(
                    {
                        "xml_id": xml_id,
                        "model": model,
                        "noupdate": current,
                        "fields": fields or None,
                    }
                )
        for child in elem:
            walk(child, current)

    walk(root, False)
    return out


def _analyze_access_csv(text):
    # security/ir.model.access.csv - the standard Odoo access-control table
    # (one row per model x group permission set). Emitted as records shaped
    # just like the XML ones above (same model, "ir.model.access") so a
    # caller doesn't need to know the source was a CSV, not a record.
    out = []
    for row in csv.DictReader(text.splitlines()):
        xml_id = (row.get("id") or "").strip()
        if not xml_id:
            continue
        fields = {}
        for key, value in row.items():
            if key == "id" or not value:
                continue
            value = value.strip()
            if value:
                fields[key] = value
        out.append(
            {
                "xml_id": xml_id,
                "model": "ir.model.access",
                "noupdate": False,
                "fields": fields or None,
            }
        )
    return out


def analyze_module(module_path):
    views = []
    models = []
    records = []
    for dirpath, dirnames, filenames in os.walk(module_path):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for filename in filenames:
            full_path = os.path.join(dirpath, filename)
            if filename.endswith(".py"):
                try:
                    with open(full_path, "r", encoding="utf-8", errors="replace") as fh:
                        source = fh.read()
                except OSError:
                    continue
                models.extend(_analyze_python_source(source))
            elif filename.endswith(".xml"):
                try:
                    with open(full_path, "rb") as fh:
                        data = fh.read()
                except OSError:
                    continue
                views.extend(_analyze_xml_source(data))
                records.extend(_analyze_xml_records(data))
            elif filename == ACCESS_CSV_FILENAME:
                try:
                    # utf-8-sig: a BOM-prefixed file (common after a Windows
                    # edit) would otherwise turn the first header into
                    # "﻿id", silently dropping every row below.
                    with open(full_path, "r", encoding="utf-8-sig", errors="replace") as fh:
                        text = fh.read()
                except OSError:
                    continue
                records.extend(_analyze_access_csv(text))
    return json.dumps({"views": views, "models": models, "records": records})
"#;

#[derive(Debug)]
pub struct GitInfo {
    pub last_commit_hash: String,
    pub last_commit_author: String,
    pub last_commit_date: String,
    pub last_commit_name: String,
    pub last_commit_partof: String,
}

#[derive(Debug)]
pub struct OGHCollectorAnalyzer {
    version_odoo: u8,
}

impl OGHCollectorAnalyzer {
    pub fn new(version_odoo: &u8) -> OGHCollectorAnalyzer {
        OGHCollectorAnalyzer {
            version_odoo: *version_odoo,
        }
    }

    fn is_odoo_module_folder(
        &self,
        mod_path: &std::path::PathBuf,
    ) -> Result<Option<String>, io::Error> {
        if !mod_path.is_dir() {
            return Ok(None);
        };
        for entry in fs::read_dir(mod_path)? {
            let path = entry?.path();
            if !path.is_dir() {
                if path.ends_with("__manifest__.py") {
                    return Ok(Some("__manifest__.py".to_string()));
                } else if path.ends_with("__openerp__.py") {
                    return Ok(Some("__openerp__.py".to_string()));
                }
            }
        }
        Ok(None)
    }

    fn get_git_info(&self, folder_path: &std::path::PathBuf) -> Result<GitInfo, ExitStatus> {
        log::info!("Get git info...");
        let output = cmd!(
            "git",
            "--no-pager",
            "log",
            "--pretty=%H~~%an~~%aD~~%s~~%b",
            "-1",
            "--",
            "."
        )
        .dir(folder_path)
        .stdin_null()
        .read()
        .unwrap_or_else(|_| String::new());
        let re =
            Regex::new(r"([0-9a-f]+)~~([^\n]+)~~([^\n]+)~~(.+)~~(?:[\S\s]+Part-of:\s([^\n]+))?")
                .unwrap();
        let caps = re.captures(&output).unwrap();
        Ok(GitInfo {
            last_commit_hash: caps
                .get(1)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_author: caps
                .get(2)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_date: caps
                .get(3)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_name: caps
                .get(4)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
            last_commit_partof: caps
                .get(5)
                .map_or(String::new(), |m| m.as_str().trim().to_string()),
        })
    }

    fn get_git_committers(
        &self,
        folder_path: &std::path::PathBuf,
    ) -> Result<HashMap<String, CommitterActivity>, ExitStatus> {
        log::info!("Get git committer info...");
        // `clone_or_update_repo` clones with `--no-single-branch --branch <ver>`,
        // which fetches every remote branch but only ever creates a *local* ref
        // for the checked-out version. So `git branch -a` lists just one bare
        // `X.Y` local branch (the current one) plus every other version as
        // `origin/X.Y`. Matching only bare `X.Y` names here meant `pos` was
        // always 0, and the `main` fallback below always failed (no local
        // `main` ref either), leaving `log_range` empty/invalid and committers
        // silently empty for virtually every module. Read positions off the
        // `origin/*` refs instead, and use `HEAD` for the current side since
        // it's always in sync with `origin/<version>` right after clone/reset.
        let branches_output = cmd!("git", "branch", "-a", "--format=%(refname:short)")
            .dir(folder_path)
            .stdin_null()
            .read()
            .unwrap_or_else(|_| String::new());
        let version_re = Regex::new(r"^origin/([0-9]+\.[0-9]+)$").unwrap();
        let mut versions: Vec<String> = branches_output
            .lines()
            .map(|l| l.trim())
            .filter_map(|l| version_re.captures(l).map(|c| c[1].to_string()))
            .collect();
        versions.sort_by(|a, b| {
            let a_parts: Vec<u32> = a.split('.').map(|p| p.parse().unwrap_or(0)).collect();
            let b_parts: Vec<u32> = b.split('.').map(|p| p.parse().unwrap_or(0)).collect();
            a_parts.cmp(&b_parts)
        });
        versions.dedup();

        let current = oghutils::version::odoo_version_u8_to_string(&self.version_odoo);

        let log_range = if let Some(pos) = versions.iter().position(|v| v == &current) {
            if pos > 0 {
                format!("origin/{}..HEAD", versions[pos - 1])
            } else {
                let default_branch = ["origin/main", "origin/master"]
                    .into_iter()
                    .find(|b| branches_output.lines().any(|l| l.trim() == *b));
                match default_branch {
                    Some(default_branch) => {
                        let base = cmd!("git", "merge-base", default_branch, "HEAD")
                            .dir(folder_path)
                            .stdin_null()
                            .read()
                            .unwrap_or_else(|_| String::new());
                        let base = base.trim();
                        if base.is_empty() {
                            "HEAD".to_string()
                        } else {
                            format!("{base}..HEAD")
                        }
                    }
                    None => "HEAD".to_string(),
                }
            }
        } else {
            log::warn!(
                "get_git_committers: current version {current} not found among origin/* branches ({versions:?}); skipping committer collection"
            );
            return Ok(HashMap::new());
        };

        // %cs is the committer date in `YYYY-MM-DD` (no time/tz), so year/month can be
        // sliced directly. Kept on the same log line as %cn so the per-committer total
        // and the per-(year, month) breakdown always come from the same commit set.
        // `--shortstat` adds one "N files changed, X insertions(+), Y deletions(-)"
        // line after each commit (either half omitted when zero); since it never
        // contains \x1f, the split_once branch below always skips it, so we only
        // need a second pass to fold it into whichever committer we last saw.
        let output = cmd!(
            "git",
            "--no-pager",
            "log",
            &log_range,
            "--shortstat",
            "--pretty=%cn%x1f%cs",
            "--",
            "."
        )
        .dir(folder_path)
        .stdin_null()
        .read()
        .unwrap_or_else(|_| String::new());

        let shortstat_re = Regex::new(
            r"^\s*\d+ files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?\s*$",
        )
        .unwrap();

        let mut committers: HashMap<String, CommitterActivity> = HashMap::new();
        let mut current_committer: Option<String> = None;
        for line in output.lines() {
            if let Some((name, date)) = line.split_once('\u{1f}') {
                current_committer = None;
                if date.len() < 7 {
                    continue;
                }
                let (Ok(year), Ok(month)) = (date[0..4].parse::<i32>(), date[5..7].parse::<i32>())
                else {
                    continue;
                };
                let activity = committers.entry(name.to_string()).or_default();
                activity.total += 1;
                *activity.periods.entry((year, month)).or_insert(0) += 1;
                current_committer = Some(name.to_string());
                continue;
            }
            if let (Some(name), Some(caps)) = (&current_committer, shortstat_re.captures(line)) {
                let activity = committers.entry(name.clone()).or_default();
                activity.insertions += caps
                    .get(1)
                    .and_then(|m| m.as_str().parse::<u32>().ok())
                    .unwrap_or(0);
                activity.deletions += caps
                    .get(2)
                    .and_then(|m| m.as_str().parse::<u32>().ok())
                    .unwrap_or(0);
            }
        }
        Ok(committers)
    }

    /// Best-effort read of `<module>/readme/<filename>` - not every module has
    /// one (used for DESCRIPTION.md, INSTALL.md, USAGE.md).
    fn read_readme_fragment(module_path: &std::path::Path, filename: &str) -> Option<String> {
        let text = fs::read_to_string(module_path.join("readme").join(filename)).ok()?;
        let text = text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    fn read_manifest(
        &self,
        org_name: &str,
        repo_name: &str,
        module_name: &str,
        manifest_path: &str,
    ) -> PyResult<ManifestInfo> {
        log::info!("Reading Manifest: {manifest_path}");
        Python::with_gil(|py| {
            let code = fs::read_to_string(manifest_path).unwrap();
            let manifest: &PyDict = py.eval(&code, None, None)?.extract()?;
            // name
            let name_opt = manifest.get_item("name");
            let name: String = if let Some(name_value) = name_opt {
                name_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                String::new()
            };
            // description - readme/DESCRIPTION.md (OCA's rendered readme fragment) wins
            // over the manifest's `description` key when present, since the manifest
            // value is often stale or just a placeholder for the generated readme.
            let description_opt = manifest.get_item("description");
            let description: String = if let Some(description_value) = description_opt {
                description_value
                    .downcast::<PyString>()?
                    .extract::<String>()?
            } else {
                String::new()
            };
            let module_dir = std::path::Path::new(manifest_path).parent();
            let description = module_dir
                .and_then(|p| Self::read_readme_fragment(p, "DESCRIPTION.md"))
                .unwrap_or(description);
            // readme/INSTALL.md and readme/USAGE.md (ponytail: module-level like
            // description, not per module_version - see the analyzer's ManifestInfo
            // note) let get_module answer "how do I install/use this module"
            // straight from what the module's own repo documents.
            let installation = module_dir
                .and_then(|p| Self::read_readme_fragment(p, "INSTALL.md"))
                .unwrap_or_default();
            let usage = module_dir
                .and_then(|p| Self::read_readme_fragment(p, "USAGE.md"))
                .unwrap_or_default();
            // author
            let author_opt = manifest.get_item("author");
            let author: String = if let Some(author_value) = author_opt {
                match author_value.downcast::<PyString>() {
                    Ok(pyval) => pyval.extract::<String>()?,
                    Err(_) => match author_value.downcast::<PyList>() {
                        Ok(pyval) => {
                            let author_vec = pyval.extract::<Vec<String>>()?;
                            author_vec.join(", ")
                        }
                        Err(_) => String::new(),
                    },
                }
            } else {
                String::new()
            };
            // website
            let website_opt = manifest.get_item("website");
            let website: String = if let Some(website_value) = website_opt {
                website_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                String::new()
            };
            // license
            let license_opt = manifest.get_item("license");
            let license: String = if let Some(license_value) = license_opt {
                license_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                "LGPL-3".to_string()
            };
            // category
            let category_opt = manifest.get_item("category");
            let category: String = if let Some(category_value) = category_opt {
                category_value.downcast::<PyString>()?.extract::<String>()?
            } else {
                "Uncategorized".to_string()
            };
            // auto_install
            let auto_install_opt = manifest.get_item("auto_install");
            let auto_install: bool = if let Some(auto_install_value) = auto_install_opt {
                match auto_install_value.downcast::<PyBool>() {
                    Ok(pyval) => pyval.extract::<bool>()?,
                    Err(_) => true,
                }
            } else {
                false
            };
            // version_odoo, version_module
            let version_opt = manifest.get_item("version");
            let version_odoo: u8;
            let version_module: String = if let Some(version_value) = version_opt {
                let version = version_value.downcast::<PyString>()?.extract::<String>()?;
                let odoo_ver = OdooVersion::new(&version, &self.version_odoo);
                version_odoo = *odoo_ver.get_version_odoo();
                odoo_ver.get_version_module().clone()
            } else {
                version_odoo = self.version_odoo;
                "0.1.0".to_string()
            };
            // application
            let application_opt = manifest.get_item("application");
            let application: bool = if let Some(application_value) = application_opt {
                application_value.downcast::<PyBool>()?.extract::<bool>()?
            } else {
                false
            };
            // installable
            let installable_opt = manifest.get_item("installable");
            let installable: bool = if let Some(installable_value) = installable_opt {
                match installable_value.downcast::<PyBool>() {
                    Ok(pyval) => pyval.extract::<bool>()?,
                    Err(_) => true,
                }
            } else {
                true
            };
            // maintainer
            let maintainer_opt = manifest.get_item("maintainer");
            let maintainer: String = if let Some(maintainer_value) = maintainer_opt {
                match maintainer_value.downcast::<PyString>() {
                    Ok(pyval) => pyval.extract::<String>()?,
                    Err(_) => match maintainer_value.downcast::<PyList>() {
                        Ok(pyval) => {
                            let maintainer_vec = pyval.extract::<Vec<String>>()?;
                            maintainer_vec.join(", ")
                        }
                        Err(_) => author.clone(),
                    },
                }
            } else {
                author.clone()
            };
            // depends
            let depends_opt = manifest.get_item("depends");
            let depends: Vec<String> = if let Some(depends_value) = depends_opt {
                depends_value
                    .downcast::<PyList>()?
                    .extract::<Vec<String>>()?
            } else {
                Vec::new()
            };
            let external_depends_opt = manifest.get_item("external_dependencies");
            let mut external_depends_python_set: HashSet<String> = HashSet::new();
            let mut external_depends_bin_set: HashSet<String> = HashSet::new();
            if let Some(external_depends_value) = external_depends_opt {
                let depends_dict = external_depends_value.downcast::<PyDict>()?;
                let depends_python_opt = depends_dict.get_item("python");
                if depends_python_opt.is_some() {
                    let python_deps = match depends_python_opt {
                        Some(py_any) => match py_any.downcast::<PyList>() {
                            Ok(pyval) => pyval,
                            Err(_) => PyList::empty(py),
                        },
                        None => PyList::empty(py),
                    };
                    for dep_name in python_deps {
                        external_depends_python_set.insert(dep_name.extract()?);
                    }
                }
                let depends_bin_opt = depends_dict.get_item("bin");
                if depends_bin_opt.is_some() {
                    let bin_deps = match depends_bin_opt {
                        Some(py_any) => match py_any.downcast::<PyList>() {
                            Ok(pyval) => pyval,
                            Err(_) => PyList::empty(py),
                        },
                        None => PyList::empty(py),
                    };
                    for dep_name in bin_deps {
                        external_depends_bin_set.insert(dep_name.extract()?);
                    }
                }
                // This is a unofficial way to get "debian" pacakage name (used by OCA CI)
                let depends_deb_opt = depends_dict.get_item("deb");
                if depends_deb_opt.is_some() {
                    let bin_deps = match depends_deb_opt {
                        Some(py_any) => match py_any.downcast::<PyList>() {
                            Ok(pyval) => pyval,
                            Err(_) => PyList::empty(py),
                        },
                        None => PyList::empty(py),
                    };
                    for dep_name in bin_deps {
                        external_depends_bin_set.insert(dep_name.extract()?);
                    }
                }
            }

            let external_depends_python: Vec<String> =
                external_depends_python_set.into_iter().collect();
            let external_depends_bin: Vec<String> = external_depends_bin_set.into_iter().collect();

            Ok(ManifestInfo {
                technical_name: module_name.into(),
                version_odoo,
                name,
                version_module,
                description,
                installation,
                usage,
                author,
                website,
                license,
                category,
                auto_install,
                application,
                installable,
                maintainer,
                git_org: org_name.into(),
                git_repo: repo_name.into(),
                depends,
                external_depends_python,
                external_depends_bin,
                folder_size: 0,
                last_commit_hash: String::new(),
                last_commit_author: String::new(),
                last_commit_name: String::new(),
                last_commit_date: String::new(),
                last_commit_partof: String::new(),
                committers: HashMap::new(),
                analysis: ModuleAnalysisInfo::default(),
            })
        })
    }

    /// Walks `module_path`'s Python/XML files (without executing any module
    /// code) to record which views it touches and which models it defines or
    /// extends, along with their public methods and `fields.X(...)`
    /// assignments. Best-effort: any failure just yields an empty analysis.
    fn analyze_module_source(&self, module_path: &std::path::Path) -> ModuleAnalysisInfo {
        let result = Python::with_gil(|py| -> PyResult<String> {
            let module = PyModule::from_code(
                py,
                ANALYZER_PY_SRC,
                "oghcollector_analyzer.py",
                "oghcollector_analyzer",
            )?;
            let analyze_fn = module.getattr("analyze_module")?;
            let module_path_str = module_path.to_string_lossy().to_string();
            analyze_fn.call1((module_path_str,))?.extract()
        });
        match result {
            Ok(json_str) => serde_json::from_str(&json_str).unwrap_or_else(|err| {
                log::warn!("Failed to parse module analysis JSON: {err}");
                ModuleAnalysisInfo::default()
            }),
            Err(err) => {
                log::warn!(
                    "Module source analysis failed for {}: {err}",
                    module_path.display()
                );
                ModuleAnalysisInfo::default()
            }
        }
    }

    pub fn get_module_info(
        &self,
        read_paths: &Vec<String>,
        repo_infos: &Vec<RepoInfo>,
    ) -> Vec<ManifestInfo> {
        let mut manifest_infos: Vec<ManifestInfo> = Vec::new();
        for repo_info in repo_infos {
            for read_path in read_paths {
                let base_path =
                    PathBuf::from(format!("{}{}", repo_info.get_clone_path(), read_path));
                log::info!("- Base Path: {}", &base_path.display());
                for entry in fs::read_dir(&base_path).unwrap() {
                    let path = entry.unwrap().path();
                    let manifest_filename_opt = self.is_odoo_module_folder(&path).unwrap();
                    if let Some(manifest_filename) = manifest_filename_opt {
                        let folder_size = get_size(&path).unwrap();
                        let git_info = self.get_git_info(&path).unwrap();
                        let committers = self.get_git_committers(&path).unwrap();
                        let manifest_path = format!("{}/{}", &path.display(), &manifest_filename);
                        let module_name = path.file_name().unwrap().to_str().unwrap();
                        let mut manifest = self
                            .read_manifest(
                                repo_info.get_org(),
                                repo_info.get_name(),
                                module_name,
                                &manifest_path,
                            )
                            .unwrap();
                        manifest.folder_size = folder_size;
                        manifest.last_commit_hash = git_info.last_commit_hash;
                        manifest.last_commit_author = git_info.last_commit_author;
                        manifest.last_commit_name = git_info.last_commit_name;
                        manifest.last_commit_date = git_info.last_commit_date;
                        manifest.last_commit_partof = git_info.last_commit_partof;
                        manifest.committers = committers;
                        manifest.analysis = self.analyze_module_source(&path);
                        manifest_infos.push(manifest);
                    }
                }
            }
        }
        manifest_infos
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_readme_fragment() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "read_readme_fragment"
        ));
        fs::create_dir_all(dir.join("readme")).unwrap();

        assert_eq!(
            OGHCollectorAnalyzer::read_readme_fragment(&dir, "DESCRIPTION.md"),
            None
        );

        fs::write(
            dir.join("readme").join("DESCRIPTION.md"),
            "  Hello module.  \n",
        )
        .unwrap();
        assert_eq!(
            OGHCollectorAnalyzer::read_readme_fragment(&dir, "DESCRIPTION.md"),
            Some("Hello module.".to_string())
        );

        fs::write(dir.join("readme").join("DESCRIPTION.md"), "   \n").unwrap();
        assert_eq!(
            OGHCollectorAnalyzer::read_readme_fragment(&dir, "DESCRIPTION.md"),
            None
        );

        // A different fragment file (INSTALL.md/USAGE.md) is read the same way.
        fs::write(dir.join("readme").join("INSTALL.md"), "pip install foo\n").unwrap();
        assert_eq!(
            OGHCollectorAnalyzer::read_readme_fragment(&dir, "INSTALL.md"),
            Some("pip install foo".to_string())
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    // Exercises the full runtime seam: analyze_module_source -> PyO3-embedded
    // Python -> JSON -> serde deserialization into the DTOs. The standalone
    // Python script and the DB replace fns are each tested separately, but
    // this is the only test that proves the two ends actually agree (a
    // field-name mismatch here would fail silently via unwrap_or_default()).
    // Every LLM-oriented addition (docstrings, signatures, decorator args,
    // field/model attrs, view_type) is asserted with a real, non-null value -
    // not just "present" - since the failure mode here is a silent None.
    #[test]
    fn test_analyze_module_source_end_to_end() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "analyze_module_source_end_to_end"
        ));
        let models_dir = dir.join("models");
        let views_dir = dir.join("views");
        fs::create_dir_all(&models_dir).unwrap();
        fs::create_dir_all(&views_dir).unwrap();

        fs::write(
            models_dir.join("res_partner.py"),
            r#"
from odoo import api, fields, models


class ResPartner(models.Model):
    """Extends res.partner with a couple of demo fields."""

    _inherit = "res.partner"
    _description = "Partner (demo)"

    x_foo = fields.Many2one("res.partner")
    x_label = fields.Char("Some Label")
    x_kind = fields.Selection([("a", "A"), ("b", "B")], string="Kind", required=True)

    @api.model
    @api.constrains("x_foo", "x_label")
    def do_it(self, force=False):
        """Does the thing and returns None."""
        pass

    def _private_helper(self):
        pass
"#,
        )
        .unwrap();

        fs::write(
            views_dir.join("res_partner_views.xml"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<odoo>
    <record model="ir.ui.view" id="view_res_partner_form_x">
        <field name="name">res.partner.form.x</field>
        <field name="model">res.partner</field>
        <field name="inherit_id" ref="base.view_partner_form"/>
        <field name="arch" type="xml">
            <xpath expr="//sheet" position="inside"/>
        </field>
    </record>
    <record model="ir.ui.view" id="view_res_partner_kind_tree">
        <field name="name">res.partner.kind.tree</field>
        <field name="model">res.partner</field>
        <field name="arch" type="xml">
            <tree>
                <field name="x_kind"/>
            </tree>
        </field>
    </record>
    <template id="portal_my_partner_kind" name="My Partner Kind">
        <div>Hello</div>
    </template>
</odoo>
"#,
        )
        .unwrap();

        let security_dir = dir.join("security");
        fs::create_dir_all(&security_dir).unwrap();
        fs::write(
            security_dir.join("security.xml"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<odoo>
    <data noupdate="1">
        <record model="res.groups" id="group_partner_kind_manager">
            <field name="name">Partner Kind Manager</field>
            <field name="category_id" ref="base.module_category_hidden"/>
            <field name="implied_ids" eval="[(4, ref('base.group_user'))]"/>
        </record>
        <record model="ir.rule" id="rule_partner_kind_manager_only" noupdate="0">
            <field name="name">Partner Kind: manager only</field>
            <field name="model_id" ref="base.model_res_partner"/>
            <field name="domain_force">[('x_kind', '!=', False)]</field>
        </record>
    </data>
</odoo>
"#,
        )
        .unwrap();
        fs::write(
            security_dir.join("ir.model.access.csv"),
            "id,name,model_id:id,group_id:id,perm_read,perm_write,perm_create,perm_unlink\n\
             access_res_partner_kind_manager,res.partner.kind.manager,model_res_partner,group_partner_kind_manager,1,1,1,0\n",
        )
        .unwrap();

        let analyzer = OGHCollectorAnalyzer::new(&17u8);
        let result = analyzer.analyze_module_source(&dir);

        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.views.len(), 3);
        let inheriting_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_form_x")
            .unwrap();
        assert_eq!(
            inheriting_view.inherit_xml_id.as_deref(),
            Some("base.view_partner_form")
        );
        assert_eq!(inheriting_view.view_type.as_deref(), Some("xpath"));

        let new_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_kind_tree")
            .unwrap();
        assert_eq!(new_view.inherit_xml_id, None);
        assert_eq!(new_view.view_type.as_deref(), Some("tree"));

        let template_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "portal_my_partner_kind")
            .unwrap();
        assert_eq!(template_view.view_type.as_deref(), Some("qweb"));

        assert_eq!(result.models.len(), 1);
        let model = &result.models[0];
        assert_eq!(model.model_name, "res.partner");
        assert_eq!(model.class_name, "ResPartner");
        assert!(!model.is_new_model);
        assert_eq!(
            model.docstring.as_deref(),
            Some("Extends res.partner with a couple of demo fields.")
        );
        let model_attrs = model.attrs.as_ref().unwrap();
        assert_eq!(model_attrs["kind"], "Model");
        assert_eq!(model_attrs["description"], "Partner (demo)");

        let foo_field = model.fields.iter().find(|f| f.name == "x_foo").unwrap();
        assert_eq!(foo_field.field_type, "Many2one");
        assert_eq!(foo_field.relation.as_deref(), Some("res.partner"));

        // A Char's first positional arg is a label, not a comodel - must stay None.
        let label_field = model.fields.iter().find(|f| f.name == "x_label").unwrap();
        assert_eq!(label_field.field_type, "Char");
        assert_eq!(label_field.relation, None);

        // Selection's option list is positional, not a kwarg - must still surface.
        let kind_field = model.fields.iter().find(|f| f.name == "x_kind").unwrap();
        assert_eq!(kind_field.field_type, "Selection");
        let kind_attrs = kind_field.attrs.as_ref().unwrap();
        assert_eq!(kind_attrs["required"], "True");
        assert_eq!(kind_attrs["string"], "'Kind'");
        assert!(kind_attrs["args"][0]
            .as_str()
            .unwrap()
            .contains("('a', 'A')"));

        // Only the public method should surface, not the underscore-prefixed helper.
        assert_eq!(model.methods.len(), 1);
        let method = &model.methods[0];
        assert_eq!(method.name, "do_it");
        assert_eq!(method.signature, "(self, force=False)");
        assert_eq!(
            method.docstring.as_deref(),
            Some("Does the thing and returns None.")
        );
        assert_eq!(
            method.decorators,
            vec![
                "api.model".to_string(),
                "api.constrains('x_foo', 'x_label')".to_string(),
            ]
        );

        // ir.ui.view records must not leak into the generic records list -
        // they're already fully covered by `views` above.
        assert!(result.records.iter().all(|r| r.model != "ir.ui.view"));

        let group = result
            .records
            .iter()
            .find(|r| r.xml_id == "group_partner_kind_manager")
            .unwrap();
        assert_eq!(group.model, "res.groups");
        // Inherited from the wrapping <data noupdate="1">.
        assert!(group.noupdate);
        let group_fields = group.fields.as_ref().unwrap();
        assert_eq!(group_fields["name"], "Partner Kind Manager");
        assert_eq!(
            group_fields["category_id"],
            "ref('base.module_category_hidden')"
        );
        assert!(group_fields["implied_ids"]
            .as_str()
            .unwrap()
            .contains("group_user"));

        let rule = result
            .records
            .iter()
            .find(|r| r.xml_id == "rule_partner_kind_manager_only")
            .unwrap();
        assert_eq!(rule.model, "ir.rule");
        // Per-record noupdate="0" must override the wrapping <data noupdate="1">.
        assert!(!rule.noupdate);
        let rule_fields = rule.fields.as_ref().unwrap();
        assert_eq!(rule_fields["domain_force"], "[('x_kind', '!=', False)]");

        let access = result
            .records
            .iter()
            .find(|r| r.xml_id == "access_res_partner_kind_manager")
            .unwrap();
        assert_eq!(access.model, "ir.model.access");
        assert!(!access.noupdate);
        let access_fields = access.fields.as_ref().unwrap();
        assert_eq!(access_fields["model_id:id"], "model_res_partner");
        assert_eq!(access_fields["group_id:id"], "group_partner_kind_manager");
        assert_eq!(access_fields["perm_read"], "1");
        assert_eq!(access_fields["perm_unlink"], "0");
    }

    // Exercises get_git_committers end to end against a real repo: two fake
    // `origin/X.Y` refs bound the log range, and two authors each contribute a
    // commit inside it, so this proves the --shortstat parsing added alongside
    // the existing %cn/%cs line-splitting (insertions/deletions folded onto
    // the last-seen committer) attributes lines to the right person.
    #[test]
    fn test_get_git_committers_counts_lines_per_author() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "get_git_committers"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let git = |args: &[&str]| {
            cmd(
                "git",
                args.iter().map(|a| a.to_string()).collect::<Vec<_>>(),
            )
            .dir(&dir)
            .stdin_null()
            .stdout_null()
            .stderr_null()
            .run()
            .unwrap();
        };

        git(&["init", "-q", "-b", "work"]);
        fs::write(dir.join("file.txt"), "1\n2\n3\n4\n5\n").unwrap();
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=Alice",
            "-c",
            "user.email=alice@test.com",
            "commit",
            "-q",
            "-m",
            "base",
        ]);
        let base_hash = cmd!("git", "rev-parse", "HEAD")
            .dir(&dir)
            .read()
            .unwrap()
            .trim()
            .to_string();
        git(&["update-ref", "refs/remotes/origin/15.0", &base_hash]);

        // Bob: 1 deletion (line "3") + 3 insertions.
        fs::write(dir.join("file.txt"), "1\n2\n4\n5\na\nb\nc\n").unwrap();
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=Bob",
            "-c",
            "user.email=bob@test.com",
            "commit",
            "-q",
            "-m",
            "bob change",
        ]);

        // Alice again: 10 insertions, 0 deletions.
        let mut content = fs::read_to_string(dir.join("file.txt")).unwrap();
        for n in 0..10 {
            content.push_str(&format!("extra{n}\n"));
        }
        fs::write(dir.join("file.txt"), content).unwrap();
        git(&["add", "-A"]);
        git(&[
            "-c",
            "user.name=Alice",
            "-c",
            "user.email=alice@test.com",
            "commit",
            "-q",
            "-m",
            "alice change 2",
        ]);
        let head_hash = cmd!("git", "rev-parse", "HEAD")
            .dir(&dir)
            .read()
            .unwrap()
            .trim()
            .to_string();
        git(&["update-ref", "refs/remotes/origin/16.0", &head_hash]);

        // version_odoo is Odoo-version*10 (see OdooVersion::new), so "16.0" is 160.
        let analyzer = OGHCollectorAnalyzer::new(&160u8);
        let committers = analyzer.get_git_committers(&dir).unwrap();

        let bob = committers.get("Bob").expect("Bob should have committed");
        assert_eq!(bob.total, 1);
        assert_eq!(bob.insertions, 3);
        assert_eq!(bob.deletions, 1);

        let alice = committers
            .get("Alice")
            .expect("Alice should have committed");
        assert_eq!(alice.total, 1, "base commit is outside origin/15.0..HEAD");
        assert_eq!(alice.insertions, 10);
        assert_eq!(alice.deletions, 0);

        fs::remove_dir_all(&dir).unwrap();
    }
}
