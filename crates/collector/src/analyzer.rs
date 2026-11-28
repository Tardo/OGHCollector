// Copyright Alexandre D. Díaz
//! `OGHCollectorAnalyzer`: walks cloned repo directories, uses **PyO3** to invoke Python to parse
//! Odoo `__manifest__.py` files, extracts `ManifestInfo`.
//!
//! Also embeds the Python extractor (`ANALYZER_PY_SRC`, stdlib-only, Python 3.8-compatible) that
//! emits raw `migration_facts`/`security_facts` (no judging) plus views/models/records/controllers;
//! records carry `demo` (their file is in the manifest's `demo`/`demo_xml`, read with
//! `ast.literal_eval`: PR heads are untrusted).
//! `ANALYSIS_VERSION` is stored per module (`module.analysis_version`) and is part of the "skip
//! unchanged module" test, so **bump it whenever the extractor, `security.rs` or `migration.rs`
//! change** — otherwise modules without new commits keep stale findings. An `#[ignore]`d
//! `calibrate` test runs every rule over real checkouts (`OGH_CALIBRATE=<dir>[:<dir>]
//! OGH_CALIBRATE_VERSION=180 cargo test -p collector calibrate -- --ignored --nocapture`); use it
//! before changing a rule's severity.
use base64::Engine;
use duct::cmd;
use pyo3::prelude::*;
use pyo3::types::*;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
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
// the module touches (security groups, record rules, cron jobs, rows from
// any `<model>.csv` data file - ir.model.access.csv, res.groups.csv, ... -
// where the filename is the model) with its resolved noupdate flag -
// so a caller can tell "does this add a new access group" or "does this
// write demo data that only loads once" without re-parsing XML itself. Also
// extracts raw `migration_facts` (old-API base classes, hand-rolled SQL,
// deprecated view/QWeb syntax, ...) - this script only reports the facts, the
// judging into severity+message happens in collector::migration, mirroring
// how collector::security judges the records/controllers here. Uses only the
// stdlib (ast, csv, re, xml.etree, xml.sax, os, json) so it runs with any Python
// available at build time - no odoo import required, and no ast.unparse
// (keeps it working on Python 3.8, which lacks it). Returns a single JSON
// string.
//
// The extra detail here (signatures, docstrings, decorator arguments, every
// field/model keyword argument) is deliberately captured beyond what the UI
// currently renders: the goal is for this data to be useful to an LLM (and
// eventually served over MCP), not just for the module detail page.
const ANALYZER_PY_SRC: &str = r##"
import ast
import csv
import json
import os
import re
import xml.etree.ElementTree as ET
import xml.sax

# Migrations and needs_upgrade hold version-specific migration scripts (old
# API, raw SQL, openerp imports - all *supposed* there to migrate old data),
# so they're never walked: flagging them would be a false migration finding.
SKIP_DIRS = {"static", "i18n", "tests", "test", "__pycache__", ".git", "migrations", "needs_upgrade"}
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


def _sql_literal(node):
    # Only a plain string constant - adjacent literals are already merged
    # into one Constant by the parser, an f-string/`%`-built query isn't
    # something we can read statically, so it's silently skipped.
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _cr_execute_sql(node):
    # Yields (sql, lineno) for every literal `cr.execute(...)`-shaped call
    # inside `node` (cr.execute / self._cr.execute / self.env.cr.execute -
    # matched by receiver name, not exact attribute chain).
    for n in ast.walk(node):
        if not (
            isinstance(n, ast.Call)
            and isinstance(n.func, ast.Attribute)
            and n.func.attr == "execute"
        ):
            continue
        receiver = _expr_repr(n.func.value) or ""
        if receiver != "cr" and not receiver.endswith(".cr") and not receiver.endswith("_cr"):
            continue
        if not n.args:
            continue
        sql = _sql_literal(n.args[0])
        if sql:
            yield sql.strip(), n.lineno


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


def _analyze_python_source(tree):
    out = []
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
                elif target_name == "_auto" and isinstance(stmt.value, ast.Constant) and isinstance(
                    stmt.value.value, bool
                ):
                    # False is the SQL-view/hand-rolled-table pattern - paired
                    # with init_sql below to tell a view (fine, ORM-agnostic
                    # by design) from a real table (the ORM never migrates
                    # its schema - see collector::migration).
                    model_attrs["auto"] = stmt.value.value
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
                if stmt.name == "init":
                    init_sql = [_truncate(sql) for sql, _lineno in _cr_execute_sql(stmt)]
                    if init_sql:
                        model_attrs["init_sql"] = init_sql
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
                "calls_sudo": _calls_method(node, "sudo"),
            }
        )
    return out


def _base_is_old_osv(base):
    # `class X(osv.osv)` / `osv.Model` / `osv.TransientModel` /
    # `osv.osv_memory` / `orm.Model` - the pre-v8 API, removed entirely in
    # modern Odoo. Matched on the base expression alone since these classes
    # don't extend models.Model and so never reach _is_model_class above.
    if isinstance(base, ast.Attribute) and isinstance(base.value, ast.Name):
        return base.value.id in ("osv", "orm") and base.attr in (
            "osv",
            "Model",
            "TransientModel",
            "AbstractModel",
            "osv_memory",
        )
    return isinstance(base, ast.Name) and base.id in ("osv", "orm")


def _analyze_migration_facts(tree, rel_path):
    # One flat list of raw facts (not yet judged/worded) for collector's Rust
    # side to turn into severity+message findings - keeps this script a pure
    # extractor, matching the split already used for security warnings
    # (see collector::security / collector::migration). `file`/`line` come
    # free from the AST node that triggered each fact.
    facts = []
    for node in ast.walk(tree):
        if isinstance(node, ast.ClassDef):
            for base in node.bases:
                if _base_is_old_osv(base):
                    facts.append(
                        {
                            "kind": "old_api_base",
                            "context": node.name,
                            "detail": _expr_repr(base),
                            "file": rel_path,
                            "line": node.lineno,
                        }
                    )
                    break
            old_decorated = []
            for stmt in node.body:
                if isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)) and _is_model_class(node):
                    if any(_expr_repr(d) in ("api.multi", "api.one") for d in stmt.decorator_list):
                        old_decorated.append(stmt)
                    if stmt.name in ("fields_view_get", "name_get"):
                        facts.append(
                            {
                                "kind": stmt.name,
                                "context": node.name,
                                "detail": None,
                                "file": rel_path,
                                "line": stmt.lineno,
                            }
                        )
                    continue
                if not isinstance(stmt, ast.Assign):
                    continue
                for target in stmt.targets:
                    if isinstance(target, ast.Name) and target.id in ("_columns", "_defaults"):
                        facts.append(
                            {
                                "kind": "old_style_field_dict",
                                "context": node.name,
                                "detail": target.id,
                                "file": rel_path,
                                "line": stmt.lineno,
                            }
                        )
                    elif isinstance(target, ast.Name) and target.id == "_sql_constraints":
                        facts.append(
                            {
                                "kind": "sql_constraints",
                                "context": node.name,
                                "detail": None,
                                "file": rel_path,
                                "line": stmt.lineno,
                            }
                        )
            if old_decorated:
                # One fact per class, not per method: it's a single mechanical
                # cleanup, listing every method would only bury other notes.
                facts.append(
                    {
                        "kind": "api_multi_one",
                        "context": node.name,
                        "detail": str(len(old_decorated)),
                        "file": rel_path,
                        "line": old_decorated[0].lineno,
                    }
                )
        elif isinstance(node, ast.Import):
            for alias in node.names:
                if alias.name == "openerp" or alias.name.startswith("openerp."):
                    facts.append(
                        {
                            "kind": "openerp_import",
                            "context": None,
                            "detail": alias.name,
                            "file": rel_path,
                            "line": node.lineno,
                        }
                    )
        elif isinstance(node, ast.ImportFrom):
            if node.module and (node.module == "openerp" or node.module.startswith("openerp.")):
                facts.append(
                    {
                        "kind": "openerp_import",
                        "context": None,
                        "detail": node.module,
                        "file": rel_path,
                        "line": node.lineno,
                    }
                )
        elif (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr in ("trg_validate", "user_has_groups")
        ):
            facts.append(
                {
                    "kind": "workflow_call" if node.func.attr == "trg_validate" else "user_has_groups",
                    "context": None,
                    "detail": _expr_repr(node.func),
                    "file": rel_path,
                    "line": node.lineno,
                }
            )
    for sql, lineno in _cr_execute_sql(tree):
        if re.search(r"\b(insert|update|delete)\b", sql, re.IGNORECASE):
            facts.append(
                {
                    "kind": "raw_sql_write",
                    "context": None,
                    "detail": _truncate(sql),
                    "file": rel_path,
                    "line": lineno,
                }
            )
    return facts


# Interpolating any of these into SQL is safe: psycopg2.sql / odoo.tools.SQL
# composables and numeric casts can't carry attacker-controlled SQL text.
SQL_SAFE_CALLS = {"Identifier", "SQL", "Literal", "AsIs", "identifier", "int", "float", "len", "bool"}
# The ORM's query builders: they validate field names, operators and order
# specs (regex_order) and keep values as query parameters, so their output is
# safe to splice whatever the caller sent (core 16 account's cumulated
# balance orders and filters by the context through them).
ORM_SQL_BUILDERS = {"_where_calc", "_generate_order_by", "_generate_order_by_inner", "_order_to_sql", "get_sql"}
# Names a model method can't receive from an RPC caller (old-API `uid` is
# set by the server).
NON_RPC_PARAMS = {"self", "cls", "cr", "uid", "registry", "env"}
# Markers for an already-reviewed hit: pylint's own check names, bandit's
# nosec, ruff's S608 and a bare `# noqa`. `# noqa: E501` (line length, all
# over OCA) must not count. Only pylint's disable has block scope.
INLINE_SUPPRESS_RE = re.compile(
    r"#.*(pylint:\s*disable=[\w, -]*(sql-injection|eval-used|exec-used)|nosec|noqa\b(?!\s*:)|noqa\s*:.*\bS608\b)"
)
BLOCK_SUPPRESS_RE = re.compile(r"^\s*#\s*pylint:\s*disable=[\w, -]*sql-injection")


def _safe(fn, *args, default=None):
    # One unexpected AST shape costs that file's facts, never the whole
    # module's analysis (which would otherwise be stored empty and stamped
    # as done until the next commit).
    try:
        return fn(*args)
    except Exception:
        return [] if default is None else default


def _sql_value_safe(node):
    # pylint-odoo's allow-list plus constants: private attributes/methods
    # (self._table, self._select()) are the standard reporting-model pattern,
    # and record ids (`rec.id`, `self.ids`) are integers.
    if isinstance(node, ast.Constant):
        return True
    # `limit and " LIMIT %s" or ""`: an `and` only passes an earlier operand
    # through when it's falsy (0, "", None - harmless), so just its last one
    # matters; any operand of an `or` / branch of an `x if c else y` can win.
    if isinstance(node, ast.BoolOp):
        values = node.values[-1:] if isinstance(node.op, ast.And) else node.values
        return all(_sql_value_safe(v) for v in values)
    if isinstance(node, ast.IfExp):
        return _sql_value_safe(node.body) and _sql_value_safe(node.orelse)
    if isinstance(node, ast.Call):
        func = node.func
        name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", None)
        if name in SQL_SAFE_CALLS:
            return True
        node = func
    return isinstance(node, ast.Attribute) and (node.attr.startswith("_") or node.attr in ("id", "ids"))


# One printf conversion: `%s`, `%(name)d`, `%-10.2f`, `%%`.
FORMAT_SPEC_RE = re.compile(r"%(?:\(([^)]*)\))?[-+ #0]*(?:\*|\d+)?(?:\.(?:\*|\d+))?[hlL]?([a-zA-Z%])")
# Conversions that only accept numbers: the format raises on anything else.
NUMERIC_CONVERSIONS = set("diouxXeEfFgGc")


def _mod_values(node):
    # The values `fmt % args` splices in as text. `%d`/`%f`-style conversions
    # force a number (core website_membership's `' LIMIT %d' % limit`), so
    # they can't carry SQL.
    right = node.right
    fmt = node.left.value if isinstance(node.left, ast.Constant) and isinstance(node.left.value, str) else None
    specs = None if fmt is None else [m for m in FORMAT_SPEC_RE.finditer(fmt) if m.group(2) != "%"]
    if isinstance(right, ast.Dict):
        if specs is None:
            return list(right.values)
        textual = {m.group(1) for m in specs if m.group(2) not in NUMERIC_CONVERSIONS}
        return [
            v
            for k, v in zip(right.keys, right.values)
            if not (isinstance(k, ast.Constant) and k.value not in textual)
        ]
    if isinstance(right, ast.Tuple):
        if (
            specs is not None
            and len(specs) == len(right.elts)
            and not any(isinstance(e, ast.Starred) for e in right.elts)
        ):
            return [v for v, m in zip(right.elts, specs) if m.group(2) not in NUMERIC_CONVERSIONS]
        return list(right.elts)
    if specs is not None and len(specs) == 1 and specs[0].group(2) in NUMERIC_CONVERSIONS:
        return []
    return [right]


def _sql_risky_values(node):
    # The unsafe values a %, +, .format() or f-string expression splices into
    # a SQL string; [] when it isn't string-building or everything is safe.
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mod):
        return [v for v in _mod_values(node) if not _sql_value_safe(v)] + _sql_risky_values(node.left)
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Add):
        out = []
        for side in (node.left, node.right):
            if isinstance(side, (ast.BinOp, ast.JoinedStr, ast.Call)):
                out += _sql_risky_values(side)
            elif not _sql_value_safe(side):
                out.append(side)
        return out
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "format":
        return [
            a for a in list(node.args) + [k.value for k in node.keywords] if not _sql_value_safe(a)
        ]
    if isinstance(node, ast.JoinedStr):
        return [
            v.value
            for v in node.values
            if isinstance(v, ast.FormattedValue) and not _sql_value_safe(v.value)
        ]
    return []


def _risky_parts(node):
    # Narrows an unsafe spliced value to the operands that can actually end
    # up in the string: `ids and "AND x IN " + alias or ""` only gates on
    # `ids`, while `alias` is spliced in.
    if _sql_value_safe(node):
        return []
    if isinstance(node, ast.BoolOp):
        values = node.values[-1:] if isinstance(node.op, ast.And) else node.values
        return [p for v in values for p in _risky_parts(v)]
    if isinstance(node, ast.IfExp):
        return _risky_parts(node.body) + _risky_parts(node.orelse)
    inner = _sql_risky_values(node)
    if inner or (isinstance(node, ast.BinOp) and isinstance(node.op, (ast.Add, ast.Mod))):
        return [p for v in inner for p in _risky_parts(v)]
    return [node]


def _orm_built(node):
    # `self._where_calc(...)`, `query.get_sql()` or `", ".join(<one of them>)`.
    if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute):
        if node.func.attr in ORM_SQL_BUILDERS:
            return True
        return node.func.attr == "join" and len(node.args) == 1 and _orm_built(node.args[0])
    return False


def _is_request_payload(node):
    # The raw client payload of an HTTP request.
    r = _expr_repr(node) or ""
    return r.startswith(("request.params", "request.httprequest.form", "request.httprequest.args"))


# Calls whose result still holds their input's text (string, dict and list
# plumbing); any other call returns that function's own value, and numeric
# casts (int, float, len, bool) end the taint.
TAINT_PASS_CALLS = {
    "get", "getlist", "pop", "setdefault", "split", "rsplit", "splitlines", "partition",
    "rpartition", "strip", "lstrip", "rstrip", "lower", "upper", "title", "capitalize",
    "replace", "join", "format", "encode", "decode", "str", "list", "tuple", "set", "dict",
    "sorted", "reversed", "copy", "keys", "values", "items", "unquote", "unquote_plus",
}
REQUEST_PREFIXES = ("request.httprequest", "request.get_json_data", "request.jsonrequest")
CONTEXT_PREFIXES = ("self._context", "self.env.context")


def _taints(node, names, context=True):
    # Whether `node`'s value carries text an RPC/HTTP caller sent: one of
    # `names` (the function's tainted locals), the request payload, or the
    # context (also sent by the client). Attribute reads (`rec.amount` is a
    # stored field, not the caller's text) and other functions' results don't.
    if isinstance(node, ast.Name):
        return node.id in names
    if isinstance(node, ast.Attribute):
        prefixes = REQUEST_PREFIXES + (CONTEXT_PREFIXES if context else ())
        return _is_request_payload(node) or (_expr_repr(node) or "").startswith(prefixes)
    if isinstance(node, (ast.Subscript, ast.Starred, ast.FormattedValue, ast.NamedExpr)):
        return _taints(node.value, names, context)
    if isinstance(node, ast.Call):
        f = node.func
        if isinstance(f, ast.Attribute) and _taints(f, names, context):
            return True
        name = f.attr if isinstance(f, ast.Attribute) else getattr(f, "id", None)
        if name not in TAINT_PASS_CALLS:
            return False
        args = list(node.args) + [k.value for k in node.keywords]
        return (isinstance(f, ast.Attribute) and _taints(f.value, names, context)) or any(
            _taints(a, names, context) for a in args
        )
    if isinstance(node, ast.BinOp):
        if isinstance(node.op, ast.Mod):
            return _taints(node.left, names, context) or any(
                _taints(v, names, context) for v in _mod_values(node)
            )
        return isinstance(node.op, ast.Add) and (
            _taints(node.left, names, context) or _taints(node.right, names, context)
        )
    if isinstance(node, ast.BoolOp):
        values = node.values[-1:] if isinstance(node.op, ast.And) else node.values
        return any(_taints(v, names, context) for v in values)
    if isinstance(node, ast.IfExp):
        return _taints(node.body, names, context) or _taints(node.orelse, names, context)
    if isinstance(node, (ast.Tuple, ast.List, ast.Set)):
        return any(_taints(v, names, context) for v in node.elts)
    if isinstance(node, ast.JoinedStr):
        return any(_taints(v, names, context) for v in node.values)
    if isinstance(node, ast.Dict):
        return any(_taints(v, names, context) for v in node.values) or any(
            k is not None and _taints(k, names, context) for k in node.keys
        )
    if isinstance(node, (ast.ListComp, ast.SetComp, ast.GeneratorExp, ast.DictComp)):
        elts = [node.key, node.value] if isinstance(node, ast.DictComp) else [node.elt]
        return any(_taints(e, names, context) for e in elts) or any(
            _taints(g.iter, names, context) for g in node.generators
        )
    return False


def _function_params(func):
    a = func.args
    names = [x.arg for x in a.posonlyargs + a.args + a.kwonlyargs]
    names += [x.arg for x in (a.vararg, a.kwarg) if x]
    return {n for n in names if n not in NON_RPC_PARAMS}


def _suppressed(lines, node, scope=None):
    # Inline marker on the flagged statement, or - pylint's block scope - a
    # comment-only `# pylint: disable=sql-injection` earlier in the function.
    end = getattr(node, "end_lineno", None) or node.lineno
    if any(INLINE_SUPPRESS_RE.search(lines[i - 1]) for i in range(node.lineno, end + 1) if i <= len(lines)):
        return True
    return scope is not None and any(
        BLOCK_SUPPRESS_RE.search(lines[i - 1]) for i in range(scope.lineno, min(node.lineno, len(lines) + 1))
    )


def _caller_params(owner, func):
    # Only parameters of an RPC/route-reachable method count as
    # client-controlled; @api.private (17.0+) opts a method out of RPC, and
    # so does a parameter the method calls: RPC can't send a function (core
    # 9 mail.alias's migrate_to_alias takes the model's _auto_init).
    params = _function_params(func)
    exposed = (
        _is_model_class(owner)
        and not func.name.startswith("_")
        and not any((_expr_repr(d) or "").endswith("private") for d in func.decorator_list)
        and not any(isinstance(n, ast.Call) and getattr(n.func, "id", None) in params for n in ast.walk(func))
    )
    if not (exposed or _route_decorator(func)):
        return set()
    # `assert mode in (...)` / `if mode not in ALLOWED: raise` is an
    # allow-list check: that parameter can't carry arbitrary values.
    return params - {
        n.left.id
        for n in ast.walk(func)
        if isinstance(n, ast.Compare)
        and isinstance(n.left, ast.Name)
        and any(isinstance(op, (ast.In, ast.NotIn)) for op in n.ops)
    }


def _assignments(func):
    # name -> every value assigned to it in the function (one-hop tracking).
    assigned = {}
    for n in ast.walk(func):
        if isinstance(n, ast.Assign):
            for t in n.targets:
                if isinstance(t, ast.Name):
                    assigned.setdefault(t.id, []).append(n.value)
        elif isinstance(n, ast.AugAssign) and isinstance(n.target, ast.Name):
            assigned.setdefault(n.target.id, []).append(n.value)
    return assigned


def _bound(target):
    # Names an assignment/loop target (re)binds. `d[k] = v` doesn't taint all
    # of `d`: one key from the context would mark every other value read from
    # it (OCA account_financial_report's `data` wizard dict).
    if isinstance(target, ast.Name):
        return [target.id]
    if isinstance(target, (ast.Tuple, ast.List)):
        return [x for e in target.elts for x in _bound(e)]
    if isinstance(target, ast.Starred):
        return _bound(target.value)
    return []


def _flows(func):
    # (target, value) of every binding: assignments, for loops, comprehensions.
    out = []
    for n in ast.walk(func):
        if isinstance(n, ast.Assign):
            out += [(t, n.value) for t in n.targets]
        elif isinstance(n, (ast.AugAssign, ast.AnnAssign, ast.NamedExpr)) and n.value is not None:
            out.append((n.target, n.value))
        elif isinstance(n, (ast.For, ast.AsyncFor, ast.comprehension)):
            out.append((n.target, n.iter))
    return out


def _tainted_names(func, seeds, flows=None):
    # Every local holding caller input, followed through any number of
    # assignments, loops and string operations (`split = gb.split(':')`,
    # `fn = split[1] if len(split) == 2 else None`). A seed the function
    # overwrites first thing (`data = {}`) no longer holds what was sent, and
    # a name checked against an allow-list (`assert mode in ("a", "b")`,
    # `if f not in self._fields: raise`) can't carry arbitrary text.
    seeds = set(seeds)
    for st in func.body:
        if (
            isinstance(st, ast.Assign)
            and len(st.targets) == 1
            and isinstance(st.targets[0], ast.Name)
            and st.targets[0].id in seeds
            and not _taints(st.value, seeds)
        ):
            seeds.discard(st.targets[0].id)
    allowed = {
        n.left.id
        for n in ast.walk(func)
        if isinstance(n, ast.Compare)
        and isinstance(n.left, ast.Name)
        and all(isinstance(op, (ast.In, ast.NotIn)) for op in n.ops)
        and not any(_taints(c, seeds) for c in n.comparators)
    }
    names = seeds - allowed
    flows = _flows(func) if flows is None else flows
    changed = bool(names)
    while changed:
        changed = False
        for target, value in flows:
            if _taints(value, names):
                new = set(_bound(target)) - names - allowed
                if new:
                    names |= new
                    changed = True
    return names


def _self_callee(call, methods):
    # The method of this class a `self._m(...)` / `self.sudo()._m(...)` /
    # `self.with_context(...)._m(...)` call reaches, else None.
    if not (isinstance(call, ast.Call) and isinstance(call.func, ast.Attribute)):
        return None
    base = call.func.value
    while isinstance(base, (ast.Call, ast.Attribute)):
        base = base.func if isinstance(base, ast.Call) else base.value
    if isinstance(base, ast.Name) and base.id == "self":
        return methods.get(call.func.attr)
    return None


def _class_tainted(owner):
    # method name -> its locals holding caller input. Seeds are the
    # parameters of RPC/route-reachable methods, plus those of the private
    # helpers they pass that input to, transitively: core 11's read_group
    # handed `groupby` to _read_group_raw and on to
    # _read_group_process_groupby, which spliced its `:month` suffix into
    # `date_trunc('%s', ...)` (OCB #1372).
    methods = {
        f.name: f for f in owner.body if isinstance(f, (ast.FunctionDef, ast.AsyncFunctionDef))
    }
    seeds = {name: _caller_params(owner, f) for name, f in methods.items()}
    flows = {name: _flows(f) for name, f in methods.items()}
    tainted = {}
    for _ in range(len(methods) + 1):
        tainted = {name: _tainted_names(f, seeds[name], flows[name]) for name, f in methods.items()}
        changed = False
        for name, f in methods.items():
            if not tainted[name]:
                continue
            for n in ast.walk(f):
                callee = _self_callee(n, methods)
                if callee is None or callee is f:
                    continue
                a = callee.args
                positional = [x.arg for x in a.posonlyargs + a.args][1:]
                keywords = {x.arg for x in a.args + a.kwonlyargs}
                hit = {
                    p
                    for p, arg in zip(positional, n.args)
                    if not isinstance(arg, ast.Starred) and _taints(arg, tainted[name])
                }
                hit |= {k.arg for k in n.keywords if k.arg in keywords and _taints(k.value, tainted[name])}
                hit -= NON_RPC_PARAMS | seeds[callee.name]
                if hit:
                    seeds[callee.name] |= hit
                    changed = True
        if not changed:
            break
    return methods, tainted


# Literal text that reads as SQL, for fragments built away from the execute()
# that runs them (a helper returning `date_trunc('%s', col)` or a WHERE clause).
SQL_FRAGMENT_RE = re.compile(
    r"\bdate_trunc\s*\(|\bselect\s+(?:distinct\s+)?[\w\"*(]+[\w\"*().,\s]*\s+from\s+[\w\"]"
    r"|\binsert\s+into\s|\bupdate\s+[\w\"]+\s+set\s|\bdelete\s+from\s|\b(?:order|group)\s+by\s"
    r"|\bwhere\s+[\w\".]+\s*(?:=|!=|<>|<=|>=|<|>|\bin\b|\bilike\b|\blike\b|\bis\b)",
    re.I,
)
# Calls whose string argument is a message, not a query.
MESSAGE_CALLS = re.compile(r"^(_|_lt|.*(Error|Exception|Warning)|.*\.(debug|info|warning|error|exception|critical|log))$")


def _literal_text(node):
    return " ".join(
        n.value for n in ast.walk(node) if isinstance(n, ast.Constant) and isinstance(n.value, str)
    )


def _is_string_building(node):
    return (
        (isinstance(node, ast.BinOp) and isinstance(node.op, (ast.Mod, ast.Add)))
        or isinstance(node, ast.JoinedStr)
        or (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "format")
    )


def _sql_fragments(func):
    # Outermost string-building expressions with SQL-looking literal text,
    # outside messages (`_()`, exceptions, logging).
    parents = {}
    for p in ast.walk(func):
        for c in ast.iter_child_nodes(p):
            parents[c] = p
    for n in ast.walk(func):
        if not _is_string_building(n) or _is_string_building(parents.get(n)):
            continue
        if not SQL_FRAGMENT_RE.search(_literal_text(n)):
            continue
        p, message = parents.get(n), False
        while p is not None and not isinstance(p, (ast.stmt,)):
            if isinstance(p, ast.Call) and MESSAGE_CALLS.match(_expr_repr(p.func) or ""):
                message = True
            p = parents.get(p)
        if not message and not isinstance(p, ast.Raise):
            yield n


# Core models whose create/write/unlink base grants only to admins
# (group_system / group_erp_manager, who can make themselves admin anyway).
ADMIN_ONLY_MODELS = {
    "ir.model", "ir.model.fields", "ir.model.constraint", "ir.model.relation",
    "ir.model.access", "ir.rule", "ir.module.module", "ir.config_parameter",
    "ir.cron", "ir.actions.server", "res.groups",
}
ORM_WRITES = ("create", "write", "unlink")


def _admin_guarded(owner, func, node):
    # The sink runs after super().<create/write/unlink>() of an admin-only
    # core model: the ORM's access check already let only admins through
    # (core 10's IrModelFields.write renames the column after super()).
    if func.name not in ORM_WRITES:
        return False
    models = set()
    for stmt in owner.body:
        if isinstance(stmt, ast.Assign) and any(
            isinstance(t, ast.Name) and t.id in ("_name", "_inherit") for t in stmt.targets
        ):
            models.update(_const_str_list(stmt.value) or [])
    if not models & ADMIN_ONLY_MODELS:
        return False
    return any(
        isinstance(n, ast.Call)
        and isinstance(n.func, ast.Attribute)
        and n.func.attr == func.name
        and isinstance(n.func.value, ast.Call)
        and isinstance(n.func.value.func, ast.Name)
        and n.func.value.func.id == "super"
        and n.lineno < node.lineno
        for n in ast.walk(func)
    )


def _analyze_sql_injection(tree, rel_path, lines):
    facts = []
    reviewed = set()

    def tainted_kind(owner, func, node):
        return "sql_injection_admin" if _admin_guarded(owner, func, node) else "sql_injection_tainted"

    def fact(kind, owner, func, node):
        # One review item per function (core's partner merge formats five
        # queries from the same table/column names); caller-controlled ones
        # are each reported.
        if kind == "sql_injection":
            if (owner.name, func.name) in reviewed:
                return
            reviewed.add((owner.name, func.name))
        facts.append(
            {
                "kind": kind,
                "context": f"{owner.name}.{func.name}",
                "detail": _truncate(lines[node.lineno - 1].strip()) if node.lineno <= len(lines) else None,
                "file": rel_path,
                "line": node.lineno,
            }
        )

    for owner in ast.walk(tree):
        if not isinstance(owner, ast.ClassDef):
            continue
        methods, tainted_by = _class_tainted(owner)
        for func in methods.values():
            params = tainted_by[func.name]
            assigned = _assignments(func)

            def constant_name(v):
                # A local (or an item of one) only ever assigned safe values:
                # `alias = p and 'p' or 'a'`, `clause = self._get_clause(data)`,
                # `limit_clause = ' LIMIT %d' % limit`.
                if isinstance(v, ast.Subscript):
                    v = v.value
                return (
                    isinstance(v, ast.Name)
                    and v.id not in params
                    and v.id in assigned
                    and not any(_risky_parts(x) for x in assigned[v.id])
                )

            def risky_of(expr):
                return [p for v in _sql_risky_values(expr) for p in _risky_parts(v) if not constant_name(p)]

            def is_tainted(v):
                return not _orm_built(v) and _taints(v, params)

            reported = set()
            for n in ast.walk(func):
                if not (isinstance(n, ast.Call) and n.args):
                    continue
                f = n.func
                is_execute = isinstance(f, ast.Attribute) and f.attr in ("execute", "executemany") and (
                    (_expr_repr(f.value) or "").split(".")[-1] in ("cr", "_cr")
                )
                is_sql = (isinstance(f, ast.Name) and f.id == "SQL") or (
                    isinstance(f, ast.Attribute) and f.attr == "SQL"
                )
                if not (is_execute or is_sql):
                    continue
                query = n.args[0]
                for sub in ast.walk(query):
                    reported.add(id(sub))
                if _suppressed(lines, n, func):
                    continue
                risky = risky_of(query)
                if not risky and isinstance(query, ast.Name):
                    # One hop: the query string built a few lines above.
                    for value in assigned.get(query.id, []):
                        for sub in ast.walk(value):
                            reported.add(id(sub))
                        if _suppressed(lines, value):
                            continue
                        risky += risky_of(value) or ([value] if _taints(value, params) and isinstance(value, ast.Name) else [])
                # The pieces a spliced local was built from are this finding too.
                for p in risky:
                    for value in assigned.get(p.id, []) if isinstance(p, ast.Name) else []:
                        for sub in ast.walk(value):
                            reported.add(id(sub))
                if risky:
                    fact(tainted_kind(owner, func, n) if any(map(is_tainted, risky)) else "sql_injection", owner, func, n)
            # SQL text built here and run elsewhere: only worth a finding when
            # it's caller-controlled (static analysis can't follow it further).
            for n in _sql_fragments(func):
                if id(n) in reported or _suppressed(lines, n, func):
                    continue
                if any(map(is_tainted, risky_of(n))):
                    fact(tainted_kind(owner, func, n), owner, func, n)
    return facts


# A regex that removes or rewrites given HTML tags/attributes. `.*` and a
# class that excludes `>` but not `<` both run across tag boundaries and
# miss nested or malformed markup, so what they "clean" can be crafted to
# survive (OCB #1378: html_normalize's `</?o:.*?>` and `<[^>]*\s)encoding=`).
# A generic strip-all-tags (`<[^>]+>`, `<.*?>`) is text extraction, not a filter.
TAG_PATTERN_RE = re.compile(r"^(?:\(\?[a-zA-Z]+\))?(?:\((?:\?:)?)*<")
GENERIC_TAG_RE = re.compile(r"^(?:\(\?[a-zA-Z]+\))?</?\??(?:\[\^>\]|\.)[*+]\??>$")
WEAK_TAG_SPAN_RE = re.compile(r"\.[*+]|\[\^(?=[^\]]*>)(?![^\]]*<)[^\]]*\]")


def _weak_tag_filter(pattern):
    return (
        isinstance(pattern, str)
        and TAG_PATTERN_RE.match(pattern) is not None
        and GENERIC_TAG_RE.match(pattern) is None
        and WEAK_TAG_SPAN_RE.search(pattern) is not None
    )


def _analyze_regex_html(tree, rel_path, lines):
    def pattern_of(node):
        # `r"<script.*?>"` or `re.compile(r"...")`.
        if isinstance(node, ast.Constant):
            return node.value
        if isinstance(node, ast.Call) and _expr_repr(node.func) == "re.compile" and node.args:
            return pattern_of(node.args[0])
        return None

    compiled = {}
    for n in ast.walk(tree):
        if isinstance(n, ast.Assign) and pattern_of(n.value) is not None and isinstance(n.value, ast.Call):
            for t in n.targets:
                key = _expr_repr(t)
                if key:
                    compiled[key] = n.value.args[0].value if isinstance(n.value.args[0], ast.Constant) else None
    facts = []
    for n in ast.walk(tree):
        if not (isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr in ("sub", "subn")):
            continue
        if _expr_repr(n.func.value) == "re":
            pattern, repl = (pattern_of(n.args[0]) if n.args else None), (n.args[1] if len(n.args) > 1 else None)
        else:
            receiver = n.func.value
            pattern = pattern_of(receiver) if isinstance(receiver, ast.Call) else compiled.get(_expr_repr(receiver))
            repl = n.args[0] if n.args else None
        if not (isinstance(repl, ast.Constant) and isinstance(repl.value, str)) or not _weak_tag_filter(pattern):
            continue
        if _suppressed(lines, n):
            continue
        facts.append(
            {
                "kind": "regex_html_filter",
                "context": _truncate(pattern),
                "detail": _truncate(lines[n.lineno - 1].strip()) if n.lineno <= len(lines) else None,
                "file": rel_path,
                "line": n.lineno,
            }
        )
    return facts


# Behind .sudo(), records a caller picks (browse/search) or values a caller
# passes whole (write/create) skip record rules and access rights.
SUDO_PICK_CALLS = {"browse", "search", "search_read", "search_fetch"}
SUDO_WRITE_CALLS = {"write", "create"}
# Ways input reaches a call unchanged: casts, `kw.get("id")`, indexing.
PASS_THROUGH_CALLS = {"int", "str", "float", "list", "tuple", "set", "get", "split", "strip"}
# A search on an unguessable or public key authenticates the caller (a
# capability link) or reads reference data; one scoped to the current user
# only finds their own records.
CAPABILITY_RE = re.compile(r"token|uuid|hash|secret|signature|key|code|endpoint", re.I)


def _direct_input(node, params):
    # Caller input reaching `node` as is, not the result of some other call.
    if isinstance(node, ast.Name):
        return node.id in params
    if _is_request_payload(node) or (_expr_repr(node) or "").startswith(
        ("request.get_json_data", "request.jsonrequest")
    ):
        return True
    if isinstance(node, (ast.List, ast.Tuple, ast.Set)):
        return any(_direct_input(e, params) for e in node.elts)
    if isinstance(node, ast.Subscript):
        return _direct_input(node.value, params)
    if isinstance(node, ast.BinOp):
        return _direct_input(node.left, params) or _direct_input(node.right, params)
    if isinstance(node, ast.IfExp):
        return _direct_input(node.body, params) or _direct_input(node.orelse, params)
    if isinstance(node, ast.BoolOp):
        return any(_direct_input(v, params) for v in node.values)
    if isinstance(node, ast.Call):
        f = node.func
        name = f.attr if isinstance(f, ast.Attribute) else getattr(f, "id", None)
        if name in PASS_THROUGH_CALLS:
            return (isinstance(f, ast.Attribute) and _direct_input(f.value, params)) or any(
                _direct_input(a, params) for a in node.args
            )
    return False


# In an if/assert test: ownership (the current user, the session), a token or
# the published flag is checked before the record is used.
OWNERSHIP_MARKERS = ("env.user", "env.uid", "session", "token", "published")


def _caller_chosen(node, func, params, assigned):
    # The value a fetched record is matched against is the caller's own
    # pick (`employee = sudo().browse(employee_id)` with no PIN asked):
    # nothing is authenticated. A local some other test also checks
    # (`employee.pin != pin_code`) counts as authenticated.
    if _direct_input(node, params):
        return True
    if not (isinstance(node, ast.Name) and assigned and node.id in assigned):
        return False
    if not any(_direct_input(x, params) for v in assigned[node.id] for x in ast.walk(v)):
        return False
    tests = [
        t for t in ast.walk(func)
        if isinstance(t, (ast.If, ast.IfExp, ast.Assert))
        and any(isinstance(x, ast.Name) and x.id == node.id for x in ast.walk(t.test))
    ]
    return len(tests) < 2


CONTAINER_FIELDS = {"config_id", "pos_config_id", "company_id", "website_id", "session_id", "event_id"}


def _owner_field(node):
    # `order.config_id.id` -> "config_id"
    while isinstance(node, ast.Attribute) and node.attr == "id" and isinstance(node.value, ast.Attribute):
        node = node.value
    return node.attr


def _checks_access(func, params=(), assigned=None):
    # Calls an access check (check_access_rule, has_access, _document_check_
    # access, _assert_portal_write_access, has_group, consteq, a token/hash/
    # signature check/validate/verify helper) or tests
    # ownership (`order.id == request.session.get(...)`, `x.user_id ==
    # request.env.user`, `partner.website_published`).
    # A check passed as an argument (`no_create=not user.has_group(...)`)
    # picks an option, it guards nothing (odoo 20 /mail/partner/from_email).
    as_argument = {
        id(x)
        for c in ast.walk(func)
        if isinstance(c, ast.Call)
        for k in c.keywords
        for x in ast.walk(k.value)
    }
    for n in ast.walk(func):
        if isinstance(n, ast.Call) and id(n) not in as_argument:
            name = (_expr_repr(n.func) or "").split(".")[-1]
            if name in ("has_group", "user_has_groups", "consteq") or name.startswith(("_can_", "can_")) or (
                any(w in name for w in ("access", "token", "hash", "signature", "jwt"))
                and any(w in name for w in ("check", "assert", "has_", "valid", "verify", "get_access", "with_access"))
            ):
                return True
        # `if rec.employee_id != employee: return`: the fetched record is
        # matched against what the caller was authenticated as. Matching
        # its container (`order.config_id != pos_config`) still lets the
        # caller pick any record in it (odoo 19 pos_self_order_pine_labs).
        if isinstance(n, ast.If) and any(
            isinstance(c, ast.Compare)
            and any(isinstance(op, (ast.NotEq, ast.IsNot, ast.NotIn)) for op in c.ops)
            and isinstance(c.left, ast.Attribute)
            and _owner_field(c.left) not in CONTAINER_FIELDS
            and not any(isinstance(x, ast.Constant) or _caller_chosen(x, func, params, assigned) for x in c.comparators)
            for c in ast.walk(n.test)
        ) and any(isinstance(b, (ast.Raise, ast.Return)) for b in n.body):
            return True
        if isinstance(n, (ast.If, ast.IfExp, ast.Assert, ast.While)) and any(
            m in (_expr_repr(x) or "") for x in ast.walk(n.test) for m in OWNERSHIP_MARKERS
        ):
            return True
    return False


# Many2ones to catalog/configuration data: filtering on them reads what
# anyone may see (published products, a website's settings).
REFERENCE_FK_RE = re.compile(
    r"^(product|product_tmpl|website|company|country|state|currency|lang|uom|pricelist|categ|category|"
    r"tag|attribute|config|pos_config|slide_channel|channel|survey|event|event_ticket|forum|blog|job|"
    r"question|page|menu|template|parent)(_ids?)?$|_(categ|category|tag|type|template)_ids?$"
)


# Catalog/configuration models: picking one by id reads what the route
# shows anyway (a product, a booth category, a self-order preset).
REFERENCE_MODEL_RE = re.compile(
    r"^(product\.|website(\.|$)|res\.(country|currency|lang)|uom\.|ir\.actions\.|event\.(event|booth)$|pos\.preset$"
    r"|survey\.survey$|delivery\.carrier$|payment\.(provider|method)$|pos\.payment\.method$|im_livechat\.channel$)"
    r"|\.(category|categ|tag|type|template|ticket)$"
)


def _picks_by_id(domain, params):
    # The caller picks the records: the whole domain is theirs, or it
    # filters `id`, a sequential `reference`/`name` (odoo 19 payment_xendit
    # charged any transaction found by its reference) or a many2one to a
    # document (`pos_order_id`, `partner_id`) on their value. Filtering other fields reads reference
    # data. odoo 20 pos_self_order_event handed out the registrations of
    # any order via `('pos_order_id', '=', int(order_id))`.
    if isinstance(domain, ast.Name) and domain.id in params:
        return True
    return any(
        isinstance(t, ast.Tuple)
        and len(t.elts) == 3
        and isinstance(t.elts[0], ast.Constant)
        and isinstance(t.elts[0].value, str)
        and (
            t.elts[0].value in ("id", "reference", "name")
            or (t.elts[0].value.endswith(("_id", "_id.id")) and not REFERENCE_FK_RE.search(t.elts[0].value.removesuffix(".id").split(".")[-1]))
        )
        and _direct_input(t.elts[2], params)
        for t in ast.walk(domain)
    )


OWNER_FIELDS = ("visitor_id", "user_id", "create_uid", "partner_id", "employee_id")


def _scoped_domain(node, params=()):
    for n in ast.walk(node):
        # `('visitor_id', '=', visitor_sudo.id)`: the caller's own records.
        if (
            isinstance(n, ast.Tuple)
            and len(n.elts) == 3
            and isinstance(n.elts[0], ast.Constant)
            and n.elts[0].value in OWNER_FIELDS
            and not _direct_input(n.elts[2], params)
        ):
            return True
        if isinstance(n, ast.Constant) and isinstance(n.value, str) and CAPABILITY_RE.search(n.value):
            return True
        if any(x in (_expr_repr(n) or "") for x in ("env.user", "env.uid", "session.uid", "self._uid")):
            return True
    return False


def _elevated_env(node, route):
    # Records looked up past the caller's own rights: `.sudo()`,
    # `.with_user(...)`, or in a route `<record>.env[...]`, the env of a
    # record the controller fetched elevated (`order_sudo.env`, a config
    # switched to its kiosk user: OCB/odoo 18 pos_self_order_razorpay picked
    # any order by id through `pos_config.env['pos.order']`).
    for x in ast.walk(node):
        if isinstance(x, ast.Call) and isinstance(x.func, ast.Attribute) and x.func.attr in ("sudo", "with_user"):
            if x.func.attr == "sudo" and x.args and isinstance(x.args[0], ast.Constant) and not x.args[0].value:
                continue
            return True
        if (
            route
            and isinstance(x, ast.Attribute)
            and x.attr == "env"
            and isinstance(x.value, ast.Name)
            and x.value.id not in ("request", "self", "http")
        ):
            return True
    return False


def _caller_browse(node, params):
    # `request.env[model].browse(res_id)`, both picked by the caller.
    return (
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr == "browse"
        and node.args
        and _direct_input(node.args[0], params)
        and isinstance(node.func.value, ast.Subscript)
        and (_expr_repr(node.func.value.value) or "").endswith("env")
        and _direct_input(node.func.value.slice, params)
    )


# BaseModel's private methods that check access themselves or read no data.
ORM_CHECKED_PRIVATE = {"_update_field_translations", "_get_base_lang", "_get_field_translations"}


def _unchecked_record_calls(func, params, assigned):
    # A private method run on a record of a model and id the caller picks:
    # browse() checks nothing and private methods don't either, so the
    # caller acts on records they can't read (odoo 20 mail's
    # /mail/partner/from_email called _partner_find_from_emails_single on
    # any thread).
    records = {
        name
        for name, values in assigned.items()
        if values and all(_caller_browse(v, params) for v in values)
    }
    for n in ast.walk(func):
        if not (isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute)):
            continue
        method, target = n.func.attr, n.func.value
        if not method.startswith("_") or "access" in method or method in ORM_CHECKED_PRIVATE:
            continue
        if _caller_browse(target, params) or (isinstance(target, ast.Name) and target.id in records):
            yield n


def _analyze_sudo_input(tree, rel_path, lines):
    # Caller input that reaches a .sudo() call in an RPC-callable model method
    # or an HTTP route, in a function that checks no access itself: the caller
    # picks records (or passes the whole values dict) past access rights.
    # One fact per function. The context doesn't count here (wizards browse
    # active_ids under sudo everywhere), nor do overrides (`super().<same>`:
    # ORM/framework entry points, whose base checks access).
    facts = []
    for owner in ast.walk(tree):
        if not isinstance(owner, ast.ClassDef):
            continue
        for func in owner.body:
            if not isinstance(func, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            route = _route_decorator(func) is not None
            params = _caller_params(owner, func)
            if not (params or route) or _checks_access(func, params, _assignments(func)):
                continue
            if not route and any(
                isinstance(n, ast.Attribute)
                and n.attr == func.name
                and isinstance(n.value, ast.Call)
                and getattr(n.value.func, "id", None) == "super"
                for n in ast.walk(func)
            ):
                continue
            assigned = _assignments(func)

            def sources(v):
                return [v] + (assigned.get(v.id, []) if isinstance(v, ast.Name) else [])

            if route:
                for n in _unchecked_record_calls(func, params, assigned):
                    if not _suppressed(lines, n, func):
                        facts.append(
                            {
                                "kind": "route_record_input",
                                "context": f"{owner.name}.{func.name}",
                                "detail": _truncate(lines[n.lineno - 1].strip()) if n.lineno <= len(lines) else None,
                                "file": rel_path,
                                "line": n.lineno,
                            }
                        )
                        break
            for n in ast.walk(func):
                if not (isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.args):
                    continue
                attr, arg = n.func.attr, n.args[0]
                if not _elevated_env(n.func.value, route):
                    continue
                kind = "route_sudo_input" if route else "rpc_sudo_input"
                model = next(
                    (
                        x.slice.value
                        for x in ast.walk(n.func.value)
                        if isinstance(x, ast.Subscript) and isinstance(x.slice, ast.Constant) and isinstance(x.slice.value, str)
                    ),
                    "",
                )
                if REFERENCE_MODEL_RE.search(model):
                    continue
                if attr == "browse":
                    hit = any(_direct_input(src, params) for src in sources(arg))
                elif attr in SUDO_PICK_CALLS:
                    srcs = sources(arg)
                    hit = any(_picks_by_id(src, params) for src in srcs) and not any(
                        _scoped_domain(src, params) for src in srcs
                    )
                elif attr in SUDO_WRITE_CALLS and not route:
                    # The whole dict a caller sent, not values picked from it.
                    kind = "rpc_sudo_write"
                    hit = isinstance(arg, ast.Name) and any(
                        (isinstance(src, ast.Name) and src.id in params) or _is_request_payload(src)
                        for src in sources(arg)
                    )
                else:
                    continue
                if hit and not _suppressed(lines, n, func):
                    facts.append(
                        {
                            "kind": kind,
                            "context": f"{owner.name}.{func.name}",
                            "detail": _truncate(lines[n.lineno - 1].strip()) if n.lineno <= len(lines) else None,
                            "file": rel_path,
                            "line": n.lineno,
                        }
                    )
                    break
    return facts


def _analyze_dangerous_calls(tree, rel_path, lines):
    # Sinks that are dangerous with any non-literal input: arbitrary code
    # execution, unsafe deserialization, shell injection, disabled TLS checks.
    facts = []
    for n in ast.walk(tree):
        if not isinstance(n, ast.Call):
            continue
        name = _expr_repr(n.func) or ""
        kwargs = {k.arg: k.value for k in n.keywords if k.arg}
        literal_arg = bool(n.args) and isinstance(n.args[0], ast.Constant)
        kind = None
        if name in ("eval", "exec") and n.args and not literal_arg:
            kind = "code_eval"
        elif name in ("pickle.loads", "pickle.load", "cPickle.loads", "marshal.loads", "yaml.unsafe_load") or (
            name == "yaml.load"
            and "Safe" not in (_expr_repr(kwargs.get("Loader")) or "")
            and not (len(n.args) > 1 and "Safe" in (_expr_repr(n.args[1]) or ""))
        ):
            kind = "unsafe_deserialization"
        elif (
            name.startswith("subprocess.")
            and isinstance(kwargs.get("shell"), ast.Constant)
            and kwargs["shell"].value is True
            and not literal_arg
        ) or (name in ("os.system", "os.popen") and n.args and not literal_arg):
            kind = "shell_injection"
        elif isinstance(kwargs.get("verify"), ast.Constant) and kwargs["verify"].value is False:
            kind = "tls_verify_disabled"
        if kind and not _suppressed(lines, n):
            facts.append(
                {
                    "kind": kind,
                    "context": name,
                    "detail": _truncate(lines[n.lineno - 1].strip()) if n.lineno <= len(lines) else None,
                    "file": rel_path,
                    "line": n.lineno,
                }
            )
    return facts


def _mass_assignment(func):
    # A route handler passing the raw request payload straight into
    # write()/create(): every field of the model becomes client-settable.
    # Reassigning the kwargs name anywhere counts as filtering it first.
    kw = func.args.kwarg.arg if func.args.kwarg else None
    if kw and any(
        isinstance(n, ast.Assign) and any(isinstance(t, ast.Name) and t.id == kw for t in n.targets)
        for n in ast.walk(func)
    ):
        kw = None

    def raw(a):
        if isinstance(a, ast.Name):
            return a.id == kw
        if isinstance(a, ast.Call) and getattr(a.func, "id", None) == "dict" and a.args:
            return raw(a.args[0])
        if isinstance(a, ast.Dict):
            return any(k is None and raw(v) for k, v in zip(a.keys, a.values))
        return _is_request_payload(a)

    return any(
        isinstance(n, ast.Call)
        and isinstance(n.func, ast.Attribute)
        and n.func.attr in ("write", "create")
        and n.args
        and raw(n.args[0])
        for n in ast.walk(func)
    )


def _route_decorator(func_node):
    # @route(...) / @http.route(...) / @odoo.http.route(...), and the
    # wrapper with route's own arguments (mail's @mail_route, which carries
    # every discuss/thread route; not @iot_route: the IoT box has no
    # database nor users); a bare @route (no call) is a pure override of an
    # inherited route and also counts.
    for dec in func_node.decorator_list:
        target = dec.func if isinstance(dec, ast.Call) else dec
        name = (
            target.attr
            if isinstance(target, ast.Attribute)
            else (target.id if isinstance(target, ast.Name) else None)
        )
        if name == "route" or (name == "mail_route" and isinstance(dec, ast.Call)):
            return dec
    return None


def _session_only(func):
    # No `env` (request.env, self.env) and no call to the controller's own
    # or parent's methods: the handler can't touch the database (core 20's
    # device-fingerprint beacon only updates the session).
    for n in ast.walk(func):
        if isinstance(n, ast.Attribute) and n.attr == "env":
            return False
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute):
            base = n.func.value
            if (isinstance(base, ast.Name) and base.id == "self") or (
                isinstance(base, ast.Call) and getattr(base.func, "id", None) == "super"
            ):
                return False
    return True


def _calls_method(func_node, method_name):
    for n in ast.walk(func_node):
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == method_name:
            return True
    return False


def _reaches_method(func_node, method_name, methods, depth=3):
    # The handler, or a helper of its own class it calls as `self._x(...)`
    # (payment webhooks delegate to `self._verify_and_process`), calls it.
    if _calls_method(func_node, method_name):
        return True
    if depth == 0:
        return False
    return any(
        _reaches_method(methods[n.func.attr], method_name, methods, depth - 1)
        for n in ast.walk(func_node)
        if isinstance(n, ast.Call)
        and isinstance(n.func, ast.Attribute)
        and isinstance(n.func.value, ast.Name)
        and n.func.value.id in ("self", "cls")
        and n.func.attr in methods
        and methods[n.func.attr] is not func_node
    )


def _analyze_controllers(tree, rel_path):
    # Every HTTP endpoint a module exposes: any method decorated with
    # http.route, whatever the class inherits from (matching on the decorator
    # instead of the Controller base also catches classes extending existing
    # controllers, e.g. `class Extended(WebsiteSale)`).
    out = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef):
            continue
        methods = {f.name: f for f in node.body if isinstance(f, (ast.FunctionDef, ast.AsyncFunctionDef))}
        for stmt in node.body:
            if not isinstance(stmt, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            dec = _route_decorator(stmt)
            if dec is None:
                continue
            paths = []
            kwargs = {}
            if isinstance(dec, ast.Call):
                if dec.args:
                    paths = _const_str_list(dec.args[0])
                for kw in dec.keywords:
                    if kw.arg:
                        kwargs[kw.arg] = kw.value

            def _const(name, default=None):
                v = kwargs.get(name)
                return v.value if isinstance(v, ast.Constant) else default

            website = _const("website") is True
            auth = _const("auth")
            if auth is None and paths:
                # Odoo defaults: "user", except website routes -> "public".
                # An override route (no path) inherits the parent's auth,
                # which we can't resolve statically: left as None.
                auth = "public" if website else "user"
            csrf = kwargs.get("csrf")
            csrf = (
                csrf.value
                if isinstance(csrf, ast.Constant) and isinstance(csrf.value, bool)
                else None
            )
            out.append(
                {
                    "class_name": node.name,
                    "name": stmt.name,
                    "routes": paths,
                    "auth": auth,
                    "http_type": _const("type") or "http",
                    "methods": (
                        _const_str_list(kwargs["methods"]) if "methods" in kwargs else []
                    ),
                    "csrf": csrf,  # None = framework default (enabled)
                    "website": website,
                    "uses_sudo": _reaches_method(stmt, "sudo", methods),
                    # Portal token-validation pattern: the endpoint gates its
                    # record access on _document_check_access(..., access_token).
                    "checks_token_access": _calls_method(stmt, "_document_check_access"),
                    "mass_assignment": _safe(_mass_assignment, stmt, default=False),
                    "session_only": _safe(_session_only, stmt, default=False),
                    "signature": _signature(stmt),
                    "docstring": _truncate(ast.get_docstring(stmt, clean=True)),
                    "file": rel_path,
                    "line": stmt.lineno,
                }
            )
    return out


# Odoo's actual ir.ui.view type values - the only tags that legitimately
# describe a view. Anything else at the arch root (xpath/data, or a bare
# locator tag like <group>/<page>/<button> used by the position="..."
# shorthand instead of an explicit <xpath>) is an inheritance patch, not a
# type: the real type is the parent view's, which _resolve_inherited_view_types
# fills in below when the parent lives in the same module.
BASE_VIEW_ARCH_TAGS = {
    "form",
    "tree",
    "list",
    "kanban",
    "search",
    "calendar",
    "gantt",
    "pivot",
    "graph",
    "activity",
    "diagram",
}


def _arch_root_tag(field_el):
    # First child element inside <field name="arch" type="xml">, e.g. "form",
    # "tree", "search", "kanban" - tells an LLM what kind of view this is
    # without needing to load/parse the whole arch.
    for child in field_el:
        if isinstance(child.tag, str):
            return child.tag
    return None


def _elem_uses_attr(el, attr_names):
    # True if `el` or any descendant carries one of `attr_names` - used for
    # attrs=/states= (deprecated view syntax) and t-raw (deprecated QWeb
    # directive), both of which can appear anywhere inside an arch/template,
    # not just at the root.
    return any(any(a in child.attrib for a in attr_names) for child in el.iter())


def _view_type_from_name(name):
    # Last-resort guess for views a same-module inherit chain can't resolve:
    # by convention the type appears as a "."/"_"-separated token, usually
    # near the end ("product.template.fieldservice.form",
    # "view_partner_form_inherit_x"). Scan tokens right-to-left; only a
    # recognized base tag counts, anything else stays None.
    if not name:
        return None
    for token in reversed(name.replace("_", ".").split(".")):
        if token in BASE_VIEW_ARCH_TAGS:
            return token
    return None


def _resolve_inherited_view_types(views, module_name):
    # Inheriting views (arch root xpath/data/field) get their type from the
    # parent view, mirroring ir.ui.view's own type compute. Only parents in
    # this same module are resolvable statically; a cross-module parent falls
    # back to guessing from the name/xml_id rather than storing a bogus type.
    by_xml_id = {v["xml_id"]: v for v in views}
    prefix = module_name + "." if module_name else None
    for view in views:
        current = view
        seen = set()
        while current["view_type"] is None and current["inherit_xml_id"]:
            ref = current["inherit_xml_id"]
            if prefix and ref.startswith(prefix):
                ref = ref[len(prefix):]
            parent = by_xml_id.get(ref)
            if parent is None or id(parent) in seen:
                break
            seen.add(id(parent))
            current = parent
        if current is not view and current["view_type"]:
            view["view_type"] = current["view_type"]
        if view["view_type"] is None:
            view["view_type"] = (
                _view_type_from_name(view["name"])
                or _view_type_from_name(view["xml_id"])
                or _view_type_from_name(view["inherit_xml_id"])
            )


class _IdLineHandler(xml.sax.ContentHandler):
    # Maps every element's `id` attribute to its 1-based source line via the
    # public Locator API - ElementTree (used for the actual DOM walk below)
    # doesn't track source positions at all. First occurrence wins; id
    # collisions within one file shouldn't happen in valid Odoo XML.
    def __init__(self):
        super().__init__()
        self._locator = None
        self.lines = {}

    def setDocumentLocator(self, locator):
        self._locator = locator

    def startElement(self, name, attrs):
        el_id = attrs.get("id")
        if el_id and el_id not in self.lines and self._locator:
            self.lines[el_id] = self._locator.getLineNumber()


def _xml_line_index(data):
    handler = _IdLineHandler()
    try:
        xml.sax.parseString(data, handler)
    except Exception:
        # SAX is stricter than ET.fromstring about encoding/entities - a
        # failed index just means no line links for this file, not a dead
        # module analysis.
        return {}
    return handler.lines


def _analyze_xml_source(data, rel_path):
    out = []
    facts = []
    try:
        root = ET.fromstring(data)
    except ET.ParseError:
        return out, facts
    line_index = _xml_line_index(data)
    for record in root.iter("record"):
        if record.attrib.get("model") != "ir.ui.view":
            continue
        xml_id = record.attrib.get("id")
        if not xml_id:
            continue
        name = None
        model = None
        inherit_xml_id = None
        explicit_type = None
        arch_tag = None
        arch_field_el = None
        for field in record.findall("field"):
            fname = field.attrib.get("name")
            if fname == "name":
                name = (field.text or "").strip() or None
            elif fname == "model":
                model = (field.text or "").strip() or None
            elif fname == "inherit_id":
                inherit_xml_id = field.attrib.get("ref")
            elif fname == "type":
                explicit_type = (field.text or "").strip() or None
            elif fname == "arch":
                arch_field_el = field
                arch_tag = _arch_root_tag(field)
        # Explicit <field name="type"> wins; only a real base-view tag counts
        # as a type, anything else (xpath/data, or a bare locator tag from
        # the position="..." shorthand) is left unresolved for the post-pass.
        if explicit_type:
            view_type = explicit_type
        elif arch_tag in BASE_VIEW_ARCH_TAGS:
            view_type = arch_tag
        else:
            view_type = None
        # arch_tag (the un-resolved root tag) rather than view_type: only a
        # view actually *defining* a tree root counts, not an inheriting
        # patch (xpath/data) that merely happens to resolve to type "tree".
        line = line_index.get(xml_id)
        if arch_tag == "tree":
            facts.append(
                {"kind": "view_tree_tag", "context": xml_id, "detail": None, "file": rel_path, "line": line}
            )
        if arch_field_el is not None:
            if _elem_uses_attr(arch_field_el, ("attrs", "states")):
                facts.append(
                    {"kind": "view_attrs_states", "context": xml_id, "detail": None, "file": rel_path, "line": line}
                )
            if _elem_uses_attr(arch_field_el, ("t-raw",)):
                facts.append(
                    {"kind": "view_t_raw", "context": xml_id, "detail": None, "file": rel_path, "line": line}
                )
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
        if _elem_uses_attr(template, ("t-raw",)):
            facts.append(
                {
                    "kind": "view_t_raw",
                    "context": xml_id,
                    "detail": None,
                    "file": rel_path,
                    "line": line_index.get(xml_id),
                }
            )
        out.append(
            {
                "xml_id": xml_id,
                "name": template.attrib.get("name") or xml_id,
                "model": "ir.ui.view",
                "inherit_xml_id": template.attrib.get("inherit_id"),
                "view_type": "qweb",
            }
        )
    return out, facts


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


def _analyze_xml_records(data, rel_path):
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
    line_index = _xml_line_index(data)

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
                        "file": rel_path,
                        "line": line_index.get(xml_id),
                    }
                )
        for child in elem:
            walk(child, current)

    walk(root, False)
    return out


def _analyze_data_csv(text, model, rel_path):
    # Odoo loads any `<model>.csv` data file (ir.model.access.csv is just the
    # most common one; res.groups.csv etc. work the same way): one record per
    # row, the filename is the model. Emitted shaped just like the XML records
    # above so a caller doesn't need to know the source was a CSV. The header
    # is physical line 1, so the first data row starts at line 2.
    out = []
    try:
        for line_no, row in enumerate(csv.DictReader(text.splitlines()), start=2):
            xml_id = (row.get("id") or "").strip()
            if not xml_id:
                continue
            fields = {}
            for key, value in row.items():
                if key == "id" or not key or not value:
                    continue
                value = value.strip()
                if value:
                    fields[key] = _truncate(value)
            out.append(
                {
                    "xml_id": xml_id,
                    "model": model,
                    "noupdate": False,
                    "fields": fields or None,
                    "file": rel_path,
                    "line": line_no,
                }
            )
    except csv.Error:
        pass
    return out


def _demo_files(module_path):
    # Files the manifest loads only as demo data (`demo`; `demo_xml` up to
    # 8.0). literal_eval, never eval: PR heads come from anyone's fork.
    for name in ("__manifest__.py", "__openerp__.py"):
        path = os.path.join(module_path, name)
        if os.path.isfile(path):
            with open(path, encoding="utf-8", errors="replace") as fh:
                manifest = ast.literal_eval(fh.read())
            return {
                os.path.normpath(f).replace(os.sep, "/")
                for key in ("demo", "demo_xml")
                for f in manifest.get(key) or []
                if isinstance(f, str)
            }
    return set()


def analyze_module(module_path):
    views = []
    models = []
    records = []
    controllers = []
    migration_facts = []
    security_facts = []
    for dirpath, dirnames, filenames in os.walk(module_path):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for filename in filenames:
            full_path = os.path.join(dirpath, filename)
            # Module-folder-relative, forward-slash - joins with the module's
            # technical name (the repo-root folder) to build a source link.
            # ponytail: assumes the module folder sits directly at the repo
            # root (true for OCA-style repos, false for e.g. odoo/odoo whose
            # addons live under addons/) - fix by resolving the module's real
            # repo-relative path if that ever needs to be exact.
            rel_path = os.path.relpath(full_path, module_path).replace(os.sep, "/")
            if filename.endswith(".py"):
                try:
                    with open(full_path, "r", encoding="utf-8", errors="replace") as fh:
                        source = fh.read()
                except OSError:
                    continue
                try:
                    tree = ast.parse(source)
                except (SyntaxError, ValueError):
                    continue
                lines = source.splitlines()
                models.extend(_safe(_analyze_python_source, tree))
                controllers.extend(_safe(_analyze_controllers, tree, rel_path))
                migration_facts.extend(_safe(_analyze_migration_facts, tree, rel_path))
                security_facts.extend(_safe(_analyze_sql_injection, tree, rel_path, lines))
                security_facts.extend(_safe(_analyze_dangerous_calls, tree, rel_path, lines))
                security_facts.extend(_safe(_analyze_sudo_input, tree, rel_path, lines))
                security_facts.extend(_safe(_analyze_regex_html, tree, rel_path, lines))
            elif filename.endswith(".xml"):
                try:
                    with open(full_path, "rb") as fh:
                        data = fh.read()
                except OSError:
                    continue
                xml_views, xml_facts = _safe(_analyze_xml_source, data, rel_path, default=([], []))
                views.extend(xml_views)
                migration_facts.extend(xml_facts)
                records.extend(_safe(_analyze_xml_records, data, rel_path))
            elif filename.endswith(".csv") and "." in filename[: -len(".csv")]:
                # Odoo data CSVs are named after their model (ir.model.access
                # .csv, res.groups.csv, ...); a dotless stem can't be a model,
                # so plain asset/fixture CSVs are skipped here.
                try:
                    # utf-8-sig: a BOM-prefixed file (common after a Windows
                    # edit) would otherwise turn the first header into
                    # "﻿id", silently dropping every row below.
                    with open(full_path, "r", encoding="utf-8-sig", errors="replace") as fh:
                        text = fh.read()
                except OSError:
                    continue
                records.extend(_safe(_analyze_data_csv, text, filename[: -len(".csv")], rel_path))
    # The module folder name is its technical name, so refs like
    # "my_module.view_x" from inside my_module resolve locally too.
    _resolve_inherited_view_types(views, os.path.basename(os.path.normpath(module_path)))
    # A manifest literal_eval can't read: no demo info, records kept as data.
    demo = _safe(_demo_files, module_path, default=set())
    for r in records:
        r["demo"] = r["file"] in demo
    # Pre-Odoo-10 manifest naming - cheap to check once per module, no walk needed.
    if os.path.isfile(os.path.join(module_path, "__openerp__.py")):
        migration_facts.append(
            {
                "kind": "openerp_manifest",
                "context": None,
                "detail": None,
                "file": "__openerp__.py",
                "line": None,
            }
        )
    return json.dumps(
        {
            "views": views,
            "models": models,
            "records": records,
            "controllers": controllers,
            "migration_facts": migration_facts,
            "security_facts": security_facts,
        }
    )
"##;

fn manifest_str(manifest: &Bound<'_, PyDict>, key: &str) -> Option<String> {
    manifest
        .get_item(key)
        .ok()??
        .cast::<PyString>()
        .ok()?
        .extract()
        .ok()
}

/// The strings of a list/tuple value (or one string); anything else, none.
fn manifest_list_strs(value: &Bound<'_, PyAny>) -> Vec<String> {
    if let Ok(s) = value.cast::<PyString>() {
        return s.extract().into_iter().collect();
    }
    if !(value.is_instance_of::<PyList>() || value.is_instance_of::<PyTuple>()) {
        return Vec::new();
    }
    value
        .try_iter()
        .into_iter()
        .flatten()
        .filter_map(|v| v.ok()?.extract::<String>().ok())
        .collect()
}

fn manifest_strs(manifest: &Bound<'_, PyDict>, key: &str) -> Vec<String> {
    match manifest.get_item(key) {
        Ok(Some(v)) => manifest_list_strs(&v),
        _ => Vec::new(),
    }
}

/// A manifest parsed with `ast.literal_eval`, like Odoo's own loader: nothing
/// in it runs. For PR heads, which come from anyone's fork (`read_manifest` does the
/// same, except for Odoo < 11.0, which falls back to `eval`).
pub(crate) fn literal_manifest(path: &std::path::Path) -> Result<serde_json::Value, String> {
    const SRC: &std::ffi::CStr = c"import ast, json
def load(path):
    with open(path, encoding='utf-8', errors='replace') as fh:
        return json.dumps(ast.literal_eval(fh.read()), default=str)
";
    let json: String = Python::attach(|py| -> PyResult<String> {
        let module = PyModule::from_code(py, SRC, c"ogh_manifest.py", c"ogh_manifest")?;
        let path = path.to_string_lossy().to_string();
        module.getattr("load")?.call1((path,))?.extract()
    })
    .map_err(|err| err.to_string())?;
    match serde_json::from_str(&json) {
        Ok(manifest @ serde_json::Value::Object(_)) => Ok(manifest),
        Ok(_) => Err("not a dict".to_string()),
        Err(err) => Err(err.to_string()),
    }
}

/// Version of the analysis rules: ANALYZER_PY_SRC plus the judging in
/// collector::security and collector::migration. Stored per module, so a
/// module is re-analyzed when this changes even if its branch never gets
/// another commit. Bump it whenever any of those three change.
pub const ANALYSIS_VERSION: i32 = 6;

#[derive(Debug, Default)]
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

    pub(crate) fn is_odoo_module_folder(
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

    pub(crate) fn get_git_info(
        &self,
        folder_path: &std::path::PathBuf,
    ) -> Result<GitInfo, ExitStatus> {
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
        // No commits touching this folder (or git failed): empty info, not a panic.
        let Some(caps) = re.captures(&output) else {
            return Ok(GitInfo::default());
        };
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

    pub(crate) fn get_git_committers(
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

    /// Best-effort read of `<module>/static/description/icon.png`, base64-encoded.
    fn read_icon(module_path: &std::path::Path) -> Option<String> {
        let bytes = fs::read(module_path.join("static/description/icon.png")).ok()?;
        Some(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    pub(crate) fn read_manifest(
        &self,
        org_name: &str,
        repo_name: &str,
        module_name: &str,
        manifest_path: &str,
    ) -> PyResult<ManifestInfo> {
        log::info!("Reading Manifest: {manifest_path}");
        Python::attach(|py| {
            let code = fs::read_to_string(manifest_path)
                .map_err(|e| pyo3::exceptions::PyIOError::new_err(e.to_string()))?;
            let code = CString::new(code)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            // Like Odoo's loader (>= 11.0): nothing in the manifest runs. Older ones may hold
            // expressions `literal_eval` refuses, so only they fall back to `eval`.
            let literal = py
                .import("ast")?
                .getattr("literal_eval")?
                .call1((code.to_string_lossy().as_ref(),));
            let manifest = match literal {
                Ok(value) => value,
                Err(_) if self.version_odoo < 110 => py.eval(&code, None, None)?,
                Err(err) => return Err(err),
            }
            .cast_into::<PyDict>()?;
            // Odoo manifests often hold `False`/tuples where a string/list is expected
            // (`'website': False`): a wrong type means "unset", never a failed module.
            let name = manifest_str(&manifest, "name").unwrap_or_default();
            // description - readme/DESCRIPTION.md (OCA's rendered readme fragment) wins
            // over the manifest's `description` key when present, since the manifest
            // value is often stale or just a placeholder for the generated readme.
            let description: String = manifest_str(&manifest, "description").unwrap_or_default();
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
            // static/description/icon.png, base64-encoded so it can travel through
            // the same text column/JSON path as description/installation/usage; the
            // module page falls back to a generic icon when this is empty.
            let icon = module_dir.and_then(Self::read_icon).unwrap_or_default();
            // author
            let author: String = manifest_strs(&manifest, "author").join(", ");
            // website
            let website: String = manifest_str(&manifest, "website").unwrap_or_default();
            // license
            let license: String = if let Some(license) = manifest_str(&manifest, "license") {
                license
            } else if self.version_odoo < 90 {
                // OpenERP <= 8.0 defaulted a missing manifest license to
                // AGPL-3; Odoo 9.0 switched the default (and core) to LGPL-3.
                "AGPL-3".to_string()
            } else {
                "LGPL-3".to_string()
            };
            // category
            let category: String =
                manifest_str(&manifest, "category").unwrap_or_else(|| "Uncategorized".to_string());
            // auto_install
            let auto_install_opt = manifest.get_item("auto_install")?;
            let auto_install: bool = if let Some(auto_install_value) = auto_install_opt {
                match auto_install_value.cast::<PyBool>() {
                    Ok(pyval) => pyval.extract::<bool>()?,
                    Err(_) => true,
                }
            } else {
                false
            };
            // version_odoo, version_module
            let version_odoo: u8;
            let version_module: String = if let Some(version) = manifest_str(&manifest, "version") {
                let odoo_ver = OdooVersion::new(&version, &self.version_odoo);
                version_odoo = *odoo_ver.get_version_odoo();
                odoo_ver.get_version_module().clone()
            } else {
                version_odoo = self.version_odoo;
                "0.1.0".to_string()
            };
            // application
            let application: bool = manifest
                .get_item("application")?
                .and_then(|v| v.cast::<PyBool>().ok().map(|b| b.is_true()))
                .unwrap_or(false);
            // installable
            let installable_opt = manifest.get_item("installable")?;
            let installable: bool = if let Some(installable_value) = installable_opt {
                match installable_value.cast::<PyBool>() {
                    Ok(pyval) => pyval.extract::<bool>()?,
                    Err(_) => true,
                }
            } else {
                true
            };
            // maintainer
            let maintainer = match manifest_strs(&manifest, "maintainer").join(", ") {
                m if m.is_empty() => author.clone(),
                m => m,
            };
            // depends
            let depends = manifest_strs(&manifest, "depends");
            let mut external_depends_python_set: HashSet<String> = HashSet::new();
            let mut external_depends_bin_set: HashSet<String> = HashSet::new();
            if let Some(external_depends_value) = manifest.get_item("external_dependencies")? {
                let depends_dict = external_depends_value.cast::<PyDict>().map_err(|_| {
                    pyo3::exceptions::PyValueError::new_err("external_dependencies isn't a dict")
                })?;
                // "deb" is an unofficial way to get the "debian" package name (used by OCA CI)
                for key in ["python", "bin", "deb"] {
                    let Some(deps_value) = depends_dict.get_item(key)? else {
                        continue;
                    };
                    let deps_list = manifest_list_strs(&deps_value);
                    let target = if key == "python" {
                        &mut external_depends_python_set
                    } else {
                        &mut external_depends_bin_set
                    };
                    target.extend(deps_list);
                }
            }

            let external_depends_python: Vec<String> =
                external_depends_python_set.into_iter().collect();
            let external_depends_bin: Vec<String> = external_depends_bin_set.into_iter().collect();

            Ok(ManifestInfo {
                technical_name: module_name.into(),
                source_path: module_name.into(),
                version_odoo,
                name,
                version_module,
                description,
                installation,
                usage,
                icon,
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
                source_unchanged: false,
            })
        })
    }

    /// Walks `module_path`'s Python/XML files (without executing any module
    /// code) to record which views it touches and which models it defines or
    /// extends, along with their public methods and `fields.X(...)`
    /// assignments. Best-effort: any failure just yields an empty analysis.
    pub(crate) fn analyze_module_source(
        &self,
        module_path: &std::path::Path,
    ) -> ModuleAnalysisInfo {
        let result = Python::attach(|py| -> PyResult<String> {
            let src = CString::new(ANALYZER_PY_SRC)
                .map_err(|e| pyo3::exceptions::PyValueError::new_err(e.to_string()))?;
            let module = PyModule::from_code(
                py,
                &src,
                c"oghcollector_analyzer.py",
                c"oghcollector_analyzer",
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

    /// The module folders of a repo's read paths, in name order (the same run
    /// logs the same events in the same order everywhere): (folder, technical
    /// name, manifest file name).
    pub(crate) fn module_folders(
        &self,
        repo_info: &RepoInfo,
        read_paths: &[String],
    ) -> Vec<(PathBuf, String, String)> {
        let mut out = Vec::new();
        for read_path in read_paths {
            let base_path = PathBuf::from(format!("{}{}", repo_info.get_clone_path(), read_path));
            log::info!("- Base Path: {}", base_path.display());
            let entries = match fs::read_dir(&base_path) {
                Ok(entries) => entries,
                Err(err) => {
                    log::warn!("Can't read '{}': {err}. Skipping...", base_path.display());
                    continue;
                }
            };
            let mut paths: Vec<PathBuf> =
                entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            paths.sort();
            for path in paths {
                let Some(manifest) = self.is_odoo_module_folder(&path).ok().flatten() else {
                    continue;
                };
                let Some(name) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(str::to_string)
                else {
                    continue;
                };
                out.push((path, name, manifest));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_with_false_and_tuple_values_still_reads() {
        let dir = std::env::temp_dir().join(format!("ogh-manifest-types-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("__manifest__.py");
        fs::write(
            &path,
            "{'name': 'X', 'website': False, 'description': False, 'author': ('A', 'B'),\n 'depends': ('base', 'web'), 'external_dependencies': {'python': ('lxml',), 'bin': False}}",
        )
        .unwrap();
        let m = OGHCollectorAnalyzer::new(&170u8)
            .read_manifest("o", "r", "x", &path.to_string_lossy())
            .unwrap();
        fs::remove_dir_all(&dir).unwrap();
        assert_eq!((m.website.as_str(), m.author.as_str()), ("", "A, B"));
        assert_eq!(m.depends, ["base", "web"]);
        assert_eq!(m.external_depends_python, ["lxml"]);
    }

    #[test]
    fn read_manifest_runs_code_only_before_odoo_11() {
        let dir = std::env::temp_dir().join(format!("ogh-manifest-eval-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (path, marker) = (dir.join("__manifest__.py"), dir.join("ran"));
        let code = format!(
            "{{'name': 'X' + open({:?}, 'w').name}}",
            marker.to_string_lossy()
        );
        fs::write(&path, code).unwrap();
        let read = |v: u8| {
            OGHCollectorAnalyzer::new(&v).read_manifest("o", "r", "x", &path.to_string_lossy())
        };
        assert!(read(170).is_err());
        assert!(!marker.exists(), "17.0 manifest was executed");
        assert!(read(100).is_ok());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_literal_manifest_never_runs_code() {
        let dir = std::env::temp_dir().join(format!("ogh-manifest-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let (manifest, pwned) = (dir.join("__manifest__.py"), dir.join("pwned"));
        let attack = format!(
            "{{'name': __import__('os').system('touch {}')}}",
            pwned.display()
        );
        fs::write(&manifest, attack).unwrap();
        assert!(literal_manifest(&manifest).is_err());
        assert!(!pwned.exists());
        fs::write(
            &manifest,
            "# comment\n{'name': 'X', 'depends': ('base',), 'installable': True}",
        )
        .unwrap();
        let parsed = literal_manifest(&manifest).unwrap();
        assert_eq!(parsed["depends"][0], "base");
        assert_eq!(parsed["installable"], true);
        fs::remove_dir_all(&dir).unwrap();
    }

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

    #[test]
    fn test_read_icon() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "read_icon"
        ));
        fs::create_dir_all(dir.join("static/description")).unwrap();

        assert_eq!(OGHCollectorAnalyzer::read_icon(&dir), None);

        fs::write(
            dir.join("static/description/icon.png"),
            [0xde, 0xad, 0xbe, 0xef],
        )
        .unwrap();
        assert_eq!(
            OGHCollectorAnalyzer::read_icon(&dir),
            Some("3q2+7w==".to_string())
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

        // Migration-consideration fixtures: an `_auto = False` reporting
        // model whose init() hand-rolls a real TABLE (not a VIEW) via raw
        // SQL, plus a handful of old-API patterns in the same file - all
        // cheap to extract from one AST walk.
        fs::write(
            models_dir.join("x_report.py"),
            r#"
import openerp
from odoo import fields, models
from openerp import workflow


class XReport(models.Model):
    _name = "x.report"
    _auto = False

    partner_id = fields.Many2one("res.partner")

    def init(self):
        self._cr.execute(
            "CREATE TABLE IF NOT EXISTS x_report (id serial, partner_id integer)"
        )

    def do_sync(self):
        self.cr.execute("UPDATE x_report SET partner_id = 1")
        workflow.trg_validate(1, "x.report", 1, "confirm", self.cr)


class OldStyleThing(osv.osv):
    _name = "x.old_thing"
    _columns = {"name": fields.char("Name")}
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
    <record model="ir.ui.view" id="view_res_partner_extra">
        <field name="name">res.partner.extra</field>
        <field name="model">res.partner</field>
        <field name="inherit_id" ref="base.view_partner_form"/>
        <field name="arch" type="xml">
            <group name="some_group" position="after">
                <field name="x_kind" attrs="{'invisible': [('x_kind', '=', 'a')]}"/>
            </group>
        </field>
    </record>
    <record model="ir.ui.view" id="view_res_partner_widgets">
        <field name="name">res.partner.widgets</field>
        <field name="model">res.partner</field>
        <field name="inherit_id" ref="base.view_partner_widgets"/>
        <field name="arch" type="xml">
            <group name="dummy" position="after"/>
        </field>
    </record>
    <record model="ir.ui.view" id="view_res_partner_kind_form_named">
        <field name="name">res.partner.kind.form</field>
        <field name="model">res.partner</field>
        <field name="inherit_id" ref="base.view_partner_form"/>
        <field name="arch" type="xml">
            <xpath expr="//sheet" position="inside"/>
        </field>
    </record>
    <record model="ir.ui.view" id="view_res_partner_kind_tree_named">
        <field name="name">res_partner_kind_tree</field>
        <field name="model">res.partner</field>
        <field name="arch" type="xml">
            <group name="dummy" position="after"/>
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
    <record model="ir.ui.view" id="view_res_partner_kind_tree_x">
        <field name="name">res.partner.kind.tree.x</field>
        <field name="model">res.partner</field>
        <field name="inherit_id" ref="view_res_partner_kind_tree"/>
        <field name="arch" type="xml">
            <xpath expr="//field[@name='x_kind']" position="attributes"/>
        </field>
    </record>
    <record model="ir.ui.view" id="view_res_partner_kind_typed">
        <field name="name">res.partner.kind.typed</field>
        <field name="model">res.partner</field>
        <field name="type">search</field>
        <field name="inherit_id" ref="base.view_res_partner_filter"/>
        <field name="arch" type="xml">
            <xpath expr="//search" position="inside"/>
        </field>
    </record>
    <template id="portal_my_partner_kind" name="My Partner Kind">
        <div t-raw="raw_html"/>
        <div>Hello</div>
    </template>
</odoo>
"#,
        )
        .unwrap();

        let controllers_dir = dir.join("controllers");
        fs::create_dir_all(&controllers_dir).unwrap();
        fs::write(
            controllers_dir.join("main.py"),
            r#"
from odoo import http
from odoo.http import request


class PartnerKindController(http.Controller):
    @http.route(["/partner_kind/webhook", "/partner_kind/hook"], type="http", auth="public", methods=["POST"], csrf=False)
    def webhook(self, **kwargs):
        """Receives external webhook calls."""
        return request.env["res.partner"].sudo().search([])

    @http.route("/partner_kind/mine", auth="user", website=True)
    def my_kinds(self):
        return request.render("partner_kind.tmpl", {})

    @http.route("/partner_kind/doc/<int:doc_id>", auth="public", website=True)
    def portal_doc(self, doc_id, access_token=None):
        doc = self._document_check_access("res.partner", doc_id, access_token)
        return request.env["res.partner"].sudo().browse(doc.id)
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
        fs::write(
            security_dir.join("res.groups.csv"),
            "id,name,implied_ids:id\n\
             group_partner_kind_csv,Partner Kind (CSV),base.group_user\n",
        )
        .unwrap();
        // Dotless stem: not a model name, must be ignored entirely.
        fs::write(security_dir.join("fixtures.csv"), "id,name\nnope,Nope\n").unwrap();
        // `./` prefix: compared normalized against the records' paths.
        fs::write(
            dir.join("__manifest__.py"),
            "# comment\n{'name': 'X', 'demo': ['./security/res.groups.csv']}\n",
        )
        .unwrap();

        let analyzer = OGHCollectorAnalyzer::new(&17u8);
        let result = analyzer.analyze_module_source(&dir);

        fs::remove_dir_all(&dir).unwrap();

        assert_eq!(result.views.len(), 9);
        // Cross-module parent, but "form" appears mid-name ("res.partner.
        // form.x"): the token scan resolves it even though it's not the
        // last segment.
        let inheriting_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_form_x")
            .unwrap();
        assert_eq!(
            inheriting_view.inherit_xml_id.as_deref(),
            Some("base.view_partner_form")
        );
        assert_eq!(inheriting_view.view_type.as_deref(), Some("form"));

        // Neither name nor xml_id carry a type token, but the inherit ref
        // ("base.view_partner_form") does: last fallback in the chain.
        let ref_named_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_extra")
            .unwrap();
        assert_eq!(ref_named_view.view_type.as_deref(), Some("form"));

        // Nothing recognizable anywhere (name, xml_id, inherit ref): stays
        // None rather than storing a bogus guess.
        let unresolvable_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_widgets")
            .unwrap();
        assert_eq!(unresolvable_view.view_type, None);

        // Cross-module parent (unresolvable via inherit chain), but the
        // "."-separated name ends in a real base type: last-resort guess.
        let dot_named_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_kind_form_named")
            .unwrap();
        assert_eq!(dot_named_view.view_type.as_deref(), Some("form"));

        // Same guess, but the name is "_"-separated instead of "."-separated.
        let underscore_named_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_kind_tree_named")
            .unwrap();
        assert_eq!(underscore_named_view.view_type.as_deref(), Some("tree"));

        let new_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_kind_tree")
            .unwrap();
        assert_eq!(new_view.inherit_xml_id, None);
        assert_eq!(new_view.view_type.as_deref(), Some("tree"));

        // Parent view is in this same module: type resolved from it.
        let local_inheriting_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_kind_tree_x")
            .unwrap();
        assert_eq!(local_inheriting_view.view_type.as_deref(), Some("tree"));

        // Explicit <field name="type"> beats everything else.
        let typed_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "view_res_partner_kind_typed")
            .unwrap();
        assert_eq!(typed_view.view_type.as_deref(), Some("search"));

        let template_view = result
            .views
            .iter()
            .find(|v| v.xml_id == "portal_my_partner_kind")
            .unwrap();
        assert_eq!(template_view.view_type.as_deref(), Some("qweb"));

        // res.partner (new-API) + x.report (new-API, `_auto = False`);
        // OldStyleThing (osv.osv base) is deliberately excluded - it only
        // surfaces via migration_facts below, not here.
        assert_eq!(result.models.len(), 2);
        let model = result
            .models
            .iter()
            .find(|m| m.model_name == "res.partner")
            .unwrap();
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

        // `_auto = False` + init()'s raw SQL round-trips into model_attrs
        // ("auto"/"init_sql") - the fact collector::migration keys off of to
        // tell a hand-rolled TABLE from the common SQL-VIEW reporting pattern.
        let x_report = result
            .models
            .iter()
            .find(|m| m.model_name == "x.report")
            .unwrap();
        let x_report_attrs = x_report.attrs.as_ref().unwrap();
        assert_eq!(x_report_attrs["auto"], false);
        let init_sql = x_report_attrs["init_sql"].as_array().unwrap();
        assert_eq!(init_sql.len(), 1);
        assert!(init_sql[0]
            .as_str()
            .unwrap()
            .contains("CREATE TABLE IF NOT EXISTS"));

        // migration_facts: every kind the Python side can emit, round-tripped
        // through JSON into MigrationFactInfo.
        let fact_kinds: Vec<&str> = result
            .migration_facts
            .iter()
            .map(|f| f.kind.as_str())
            .collect();
        for expected in [
            "old_api_base",
            "old_style_field_dict",
            "openerp_import",
            "workflow_call",
            "raw_sql_write",
            "view_attrs_states",
            "view_t_raw",
        ] {
            assert!(
                fact_kinds.contains(&expected),
                "missing fact kind {expected:?} in {fact_kinds:?}"
            );
        }
        let old_api_fact = result
            .migration_facts
            .iter()
            .find(|f| f.kind == "old_api_base")
            .unwrap();
        assert_eq!(old_api_fact.context.as_deref(), Some("OldStyleThing"));
        assert_eq!(old_api_fact.detail.as_deref(), Some("osv.osv"));
        assert_eq!(old_api_fact.file.as_deref(), Some("models/x_report.py"));
        assert_eq!(old_api_fact.line, Some(23));
        let attrs_fact = result
            .migration_facts
            .iter()
            .find(|f| f.kind == "view_attrs_states")
            .unwrap();
        assert_eq!(
            attrs_fact.context.as_deref(),
            Some("view_res_partner_extra")
        );
        // Resolved via the SAX line index (by the record's `id` attribute),
        // not the AST - exercises the ElementTree-has-no-lineno workaround.
        assert_eq!(
            attrs_fact.file.as_deref(),
            Some("views/res_partner_views.xml")
        );
        assert_eq!(attrs_fact.line, Some(11));
        let t_raw_fact = result
            .migration_facts
            .iter()
            .find(|f| f.kind == "view_t_raw")
            .unwrap();
        assert_eq!(
            t_raw_fact.context.as_deref(),
            Some("portal_my_partner_kind")
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
        assert_eq!(group.file.as_deref(), Some("security/security.xml"));
        assert_eq!(group.line, Some(4));
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
        // CSV rows: file is module-relative, line is header(1) + row offset.
        assert_eq!(access.file.as_deref(), Some("security/ir.model.access.csv"));
        assert_eq!(access.line, Some(2));
        assert!(!access.demo);

        // Groups created via res.groups.csv must surface like the XML ones.
        let csv_group = result
            .records
            .iter()
            .find(|r| r.xml_id == "group_partner_kind_csv")
            .unwrap();
        assert_eq!(csv_group.model, "res.groups");
        let csv_group_fields = csv_group.fields.as_ref().unwrap();
        assert_eq!(csv_group_fields["name"], "Partner Kind (CSV)");
        assert_eq!(csv_group_fields["implied_ids:id"], "base.group_user");
        assert!(csv_group.demo);

        // fixtures.csv has no dot in its stem - not an Odoo data file.
        assert!(result.records.iter().all(|r| r.xml_id != "nope"));

        // HTTP controllers: every routed method, with resolved auth/defaults.
        assert_eq!(result.controllers.len(), 3);
        let webhook = result
            .controllers
            .iter()
            .find(|c| c.name == "webhook")
            .unwrap();
        assert_eq!(webhook.class_name, "PartnerKindController");
        assert_eq!(
            webhook.routes,
            vec!["/partner_kind/webhook", "/partner_kind/hook"]
        );
        assert_eq!(webhook.auth.as_deref(), Some("public"));
        assert_eq!(webhook.http_type, "http");
        assert_eq!(webhook.methods, vec!["POST"]);
        assert_eq!(webhook.csrf, Some(false));
        assert!(webhook.uses_sudo);
        assert!(!webhook.checks_token_access);
        assert!(!webhook.website);
        assert_eq!(webhook.signature, "(self, **kwargs)");
        assert_eq!(
            webhook.docstring.as_deref(),
            Some("Receives external webhook calls.")
        );
        // ast.FunctionDef.lineno points at `def`, not the decorator above it.
        assert_eq!(webhook.file.as_deref(), Some("controllers/main.py"));
        assert_eq!(webhook.line, Some(8));

        let my_kinds = result
            .controllers
            .iter()
            .find(|c| c.name == "my_kinds")
            .unwrap();
        assert_eq!(my_kinds.auth.as_deref(), Some("user"));
        assert_eq!(my_kinds.csrf, None); // framework default, not recorded
        assert!(my_kinds.website);
        assert!(!my_kinds.uses_sudo);
        assert!(my_kinds.methods.is_empty()); // all methods accepted

        // Portal pattern: public + sudo, but gated by _document_check_access.
        let portal_doc = result
            .controllers
            .iter()
            .find(|c| c.name == "portal_doc")
            .unwrap();
        assert!(portal_doc.uses_sudo);
        assert!(portal_doc.checks_token_access);
    }

    // Migration scripts (the `migrations/<version>/` data-migration scripts and
    // the `needs_upgrade/` upgrade scripts) are *supposed* to use old API,
    // openerp imports and raw SQL - flagging them would be a false positive in
    // the migration considerations. Both dirs are pruned from the walk, so a
    // module that only contains such scripts must yield zero migration facts,
    // while the same patterns in a normal models/ file still do.
    #[test]
    fn test_migration_scripts_are_excluded_from_facts() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "migration_scripts_excluded"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("migrations/16.0.1.0.0")).unwrap();
        fs::create_dir_all(dir.join("needs_upgrade")).unwrap();
        fs::create_dir_all(dir.join("models")).unwrap();

        // A normal model with the same old-API/raw-SQL/workflow patterns -
        // these MUST still be flagged.
        fs::write(
            dir.join("models/real.py"),
            r#"
import openerp
from odoo import models
from openerp import workflow


class Real(models.Model):
    _name = "real.model"

    def do_sync(self):
        self.cr.execute("UPDATE real.model SET x = 1")
        workflow.trg_validate(1, "real.model", 1, "confirm", self.cr)
"#,
        )
        .unwrap();

        // A migrations/ script and a needs_upgrade/ script, both carrying the
        // exact same smell - these MUST be ignored.
        fs::write(
            dir.join("migrations/16.0.1.0.0/migrate.py"),
            r#"
from openerp import workflow
from openerp osv import osv


class Old(osv.osv):
    _name = "old.migrated"

    def _run(self):
        self.cr.execute("INSERT INTO old.migrated VALUES (1)")
        workflow.trg_validate(1, "old.migrated", 1, "confirm", self.cr)
"#,
        )
        .unwrap();
        fs::write(
            dir.join("needs_upgrade/data.py"),
            r#"
import openerp


def migrate(cr, version):
    cr.execute("UPDATE old.migrated SET active = True")
    openerp.workflow.trg_validate(1, "old.migrated", 1, "confirm", cr)
"#,
        )
        .unwrap();

        let analyzer = OGHCollectorAnalyzer::new(&160u8);
        let result = analyzer.analyze_module_source(&dir);

        // The normal file's facts survive...
        let kinds: Vec<&str> = result
            .migration_facts
            .iter()
            .map(|f| f.kind.as_str())
            .collect();
        assert!(
            kinds.contains(&"openerp_import")
                && kinds.contains(&"raw_sql_write")
                && kinds.contains(&"workflow_call"),
            "normal-model patterns must still be flagged, got {kinds:?}"
        );
        // ...and none of them live inside a migration script.
        for f in &result.migration_facts {
            let file = f.file.as_deref().unwrap_or("");
            assert!(
                !file.starts_with("migrations/") && !file.starts_with("needs_upgrade/"),
                "migration script fact leaked: {:?} at {file:?}",
                f.kind
            );
        }

        fs::remove_dir_all(&dir).unwrap();
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

    // Code-level security facts: which SQL counts as caller-controlled
    // ("_tainted" = grave), which is only worth a review, and which must stay
    // silent (parametrized, allow-listed, private-attribute, suppressed).
    #[test]
    fn test_security_facts_end_to_end() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "security_facts"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("main.py"),
            r#"
import pickle, subprocess, requests, yaml
from odoo import http, models
from odoo.http import request


class Thing(models.Model):
    _name = "x.thing"

    def public_search(self, name, mode):
        assert mode in ("a", "b")
        self.env.cr.execute("SELECT 1 WHERE n = '%s'" % name)  # T1
        self.env.cr.execute(f"SELECT 1 WHERE m = {mode}")  # R1
        self.env.cr.execute("SELECT id FROM %s" % self._table)
        self.env.cr.execute("SELECT 1 WHERE n = %s", (name,))
        query = "SELECT 1 WHERE n = '%s'" % name
        self.env.cr.execute(query)  # T2
        self.env.cr.execute("SELECT 1 " + name)  # pylint: disable=sql-injection
        rec = self.browse(name)
        self.env.cr.execute("SELECT %s LIMIT %s" % (rec.amount, name and "10" or "1"))  # R2

    def _private(self, where):
        self.env.cr.execute("SELECT 1 WHERE %s" % where)  # R3
        self.env.cr.execute("SELECT 1 WHERE n = %s" % self.env.context.get("n"))  # T3
        query = self._where_calc(list(self.env.context.get("domain") or []))
        order = ", ".join(self._generate_order_by_inner(self._table, self.env.context.get("order"), query))
        sql = "SELECT 1 ORDER BY %s" % order
        self.env.cr.execute(sql)  # R4

    def set_values(self, vals):
        self.sudo().write(vals)  # S1

    def peek(self, partner_id):
        return self.env["res.partner"].sudo().browse(int(partner_id)).name  # S2

    def peek_checked(self, partner_id):
        self.check_access_rights("read")
        return self.env["res.partner"].sudo().browse(partner_id).name

    def write(self, vals):
        self.sudo().mapped("child_ids").write(vals)
        return super().write(vals)


class Ctl(http.Controller):
    @http.route("/x/update", auth="public", csrf=False, methods=["POST"])
    def update(self, **kw):
        request.env["x.thing"].sudo().browse(1).write(kw)

    @http.route("/x/beacon", auth="user", csrf=False, methods=["POST"])
    def beacon(self, fp):
        update_fingerprint(request.session, request, fp)

    @http.route("/x/safe", auth="user")
    def safe(self, **kw):
        kw = {k: kw[k] for k in ("name",) if k in kw}
        request.env["x.thing"].browse(1).write(kw)

    @http.route("/x/find", auth="public")
    def find(self, q=None):
        request.env.cr.execute("SELECT 1 WHERE n = '%s'" % q)  # T4

    @http.route("/x/doc/<int:doc_id>", auth="user")
    def doc(self, doc_id, **kw):
        return request.env["x.thing"].sudo().browse(doc_id).name  # S3

    @http.route("/x/mine/<int:doc_id>", auth="user")
    def mine(self, doc_id):
        doc = request.env["x.thing"].sudo().browse(doc_id)
        if doc.user_id != request.env.user:
            raise ValueError()

    @http.route("/x/token", auth="public")
    def by_token(self, token, stage):
        request.env["x.stage"].sudo().search([("job_ids", "=", stage)])
        return request.env["x.thing"].sudo().search([("id", "=", 1), ("access_token", "=", token)])


def helpers(data, cmd):
    eval(data)  # D1
    eval("1 + 1")
    pickle.loads(data)  # D2
    yaml.load(data, Loader=yaml.SafeLoader)
    yaml.load(data)  # D3
    subprocess.run(cmd, shell=True)  # D4
    subprocess.run(["ls"], shell=False)
    requests.get("https://x", verify=False)  # D5
"#,
        )
        .unwrap();

        let result = OGHCollectorAnalyzer::new(&170u8).analyze_module_source(&dir);
        fs::remove_dir_all(&dir).unwrap();

        let mut found: Vec<(String, String)> = result
            .security_facts
            .iter()
            .map(|f| {
                let detail = f.detail.as_deref().unwrap_or("");
                let marker = detail.rsplit("# ").next().unwrap_or("").to_string();
                (marker, f.kind.clone())
            })
            .collect();
        found.sort();
        let expected = [
            ("D1", "code_eval"),
            ("D2", "unsafe_deserialization"),
            ("D3", "unsafe_deserialization"),
            ("D4", "shell_injection"),
            ("D5", "tls_verify_disabled"),
            // R2 and R4 share R1's and R3's functions: one review item each.
            ("R1", "sql_injection"),
            ("R3", "sql_injection"),
            ("S1", "rpc_sudo_write"),
            ("S2", "rpc_sudo_input"),
            ("S3", "route_sudo_input"),
            ("T1", "sql_injection_tainted"),
            ("T2", "sql_injection_tainted"),
            ("T3", "sql_injection_tainted"),
            ("T4", "sql_injection_tainted"),
        ];
        assert_eq!(
            found,
            expected
                .iter()
                .map(|(m, k)| (m.to_string(), k.to_string()))
                .collect::<Vec<_>>()
        );

        let update = result
            .controllers
            .iter()
            .find(|c| c.name == "update")
            .unwrap();
        assert!(update.mass_assignment);
        assert!(!update.session_only);
        let beacon = result
            .controllers
            .iter()
            .find(|c| c.name == "beacon")
            .unwrap();
        assert!(beacon.session_only);
        // Reassigned (filtered) kwargs before write: not mass assignment.
        let safe = result
            .controllers
            .iter()
            .find(|c| c.name == "safe")
            .unwrap();
        assert!(!safe.mass_assignment);
    }

    // Public routes that pick documents past the visitor's rights, from real
    // odoo 18-20 fixes (before: B*, a finding; after: A*, silent), plus
    // catalog lookups and owner-scoped searches that stay silent (Q*).
    #[test]
    fn test_route_record_picks_from_real_fixes() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "route_picks"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("main.py"),
            r#"
from odoo import http
from odoo.http import request
from odoo.tools import consteq
from odoo.addons.mail.tools.discuss import mail_route


class Ctrl(http.Controller):
    @http.route("/razorpay/status", auth="public", type="jsonrpc")
    def razorpay_status(self, access_token, order_id):
        pos_config = self._verify_pos_config(access_token)
        order = pos_config.env['pos.order'].search([('id', '=', order_id), ('config_id', '=', pos_config.id)])  # B1
        order.action_pos_order_paid()

    @http.route("/razorpay/status2", auth="public", type="jsonrpc")
    def razorpay_status_fixed(self, access_token, order_id, payment_data):
        pos_config = self._verify_pos_config(access_token)
        order = pos_config.env['pos.order'].search([('id', '=', order_id), ('access_token', '=', payment_data.get('order_token'))])  # A1
        order.action_pos_order_paid()

    @http.route("/pine/status", auth="public", type="jsonrpc")
    def pine_status(self, access_token, order_id):
        pos_config = self._verify_pos_config(access_token)
        order = pos_config.env['pos.order'].browse(order_id)  # B2
        if not order.exists() or order.config_id.id != pos_config.id:
            raise NotFound()
        order.action_pos_order_paid()

    @http.route("/event/regs", auth="public", type="jsonrpc")
    def event_regs(self, access_token, order_id):
        pos_config = self._verify_pos_config(access_token)
        return pos_config.env['event.registration'].search([('pos_order_id', '=', int(order_id))])  # B3

    @http.route("/xendit/pay", auth="public", type="jsonrpc")
    def xendit_payment(self, reference, token_ref):
        tx_sudo = request.env['payment.transaction'].sudo().search([('reference', '=', reference)])  # B4
        tx_sudo._xendit_create_charge(token_ref)

    @http.route("/xendit/pay2", auth="public", type="jsonrpc")
    def xendit_payment_fixed(self, reference, token_ref, access_token):
        tx_sudo = request.env['payment.transaction'].sudo().search([('reference', '=', reference)])  # A4
        if not payment_utils.check_access_token(access_token, reference):
            raise ValidationError("no")
        tx_sudo._xendit_create_charge(token_ref)

    @mail_route("/mail/partner/from_email", methods=["POST"], type="jsonrpc", auth="user")
    def from_email(self, thread_model, thread_id, emails):
        return request.env[thread_model].browse(thread_id)._partner_find_from_emails_single(  # B5
            emails, no_create=not request.env.user.has_group("base.group_partner_manager")
        )

    @mail_route("/mail/partner/from_email2", methods=["POST"], type="jsonrpc", auth="user")
    def from_email_fixed(self, thread_model, thread_id, emails):
        thread = self._get_thread_with_access(thread_model, thread_id)
        return thread._partner_find_from_emails_single(emails)  # A5

    @http.route("/attendance/break_nopin", auth="public", type="jsonrpc")
    def update_break_nopin(self, token, attendance_id, employee_id):
        employee = request.env['hr.employee'].sudo().browse(employee_id)  # B6
        attendance = request.env['hr.attendance'].sudo().browse(attendance_id)
        if attendance.employee_id != employee:
            return {}
        attendance.write({'break': 1})

    @http.route("/attendance/break_pin", auth="public", type="jsonrpc")
    def update_break_pin(self, token, attendance_id, employee_id, pin_code):
        employee = request.env['hr.employee'].sudo().browse(employee_id)
        if employee.pin != pin_code:
            employee = request.env['hr.employee']
        attendance = request.env['hr.attendance'].sudo().browse(attendance_id)  # A6
        if attendance.employee_id != employee:
            return {}
        attendance.write({'break': 1})

    @http.route("/shop/viewed_delete", auth="public", type="jsonrpc")
    def viewed_delete(self, product_id):
        visitor_sudo = self.env["ir.http"]._get_visitor_from_request()
        domain = [("visitor_id", "=", visitor_sudo.id), ("product_id", "=", int(product_id))]
        self.env["website.track"].sudo().search(domain).unlink()  # Q1

    @http.route("/shop/rate", auth="public", type="jsonrpc")
    def rate(self, dm_id):
        return self.env["delivery.carrier"].sudo().browse(int(dm_id)).exists()  # Q2

    @http.route("/translate", auth="user", type="jsonrpc")
    def translate(self, model, record_id, field_name, translations):
        record = request.env[model].browse(record_id)
        return record._update_field_translations(field_name, translations)  # Q3
"#,
        )
        .unwrap();
        let result = OGHCollectorAnalyzer::new(&200u8).analyze_module_source(&dir);
        fs::remove_dir_all(&dir).unwrap();
        let mut found: Vec<(String, String)> = result
            .security_facts
            .iter()
            .map(|f| {
                let detail = f.detail.as_deref().unwrap_or("");
                let marker = detail.rsplit("# ").next().unwrap_or("").to_string();
                (marker, f.kind.clone())
            })
            .collect();
        found.sort();
        let expected = [
            ("B1", "route_sudo_input"),
            ("B2", "route_sudo_input"),
            ("B3", "route_sudo_input"),
            ("B4", "route_sudo_input"),
            ("B5", "route_record_input"),
            ("B6", "route_sudo_input"),
        ];
        assert_eq!(
            found,
            expected
                .iter()
                .map(|(m, k)| (m.to_string(), k.to_string()))
                .collect::<Vec<_>>()
        );
    }

    // Caller input followed into private helpers and SQL built away from its
    // execute() (OCB #1372's read_group spec), regex HTML filters (OCB
    // #1378's html_normalize), and the real false positives that must stay
    // review items: numeric `%d`, a private helper's result, a parameter the
    // method overwrites, a callable parameter (not RPC-reachable).
    #[test]
    fn test_taint_through_helpers_and_regex_html_filters() {
        let dir = std::env::temp_dir().join(format!(
            "oghcollector_analyzer_test_{}_{}",
            std::process::id(),
            "taint_helpers"
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("main.py"),
            r#"
import re
from odoo import models, _

DOCTYPE = re.compile(r'(<[^>]*\s)(encoding=(["\'][^"\']*?["\']|[^\s\n\r>]+)(\s[^>]*|/)?>)', re.I)


class Thing(models.Model):
    _name = "x.thing"

    def read_group(self, domain, fields, groupby, lazy=True):
        return self._read_group_raw(domain, fields, groupby, lazy=lazy)

    def _read_group_raw(self, domain, fields, groupby, lazy=True):
        query = self._where_calc(domain)
        groupby_list = groupby[:1] if lazy else groupby
        return [self._process_groupby(gb, query) for gb in groupby_list]

    def _process_groupby(self, gb, query):
        split = gb.split(':')
        fn = split[1] if len(split) == 2 else None
        field = self._inherits_join_calc(self._table, split[0], query)
        return "date_trunc('%s', %s)" % (fn or 'month', field)  # T1

    def _unreached(self, gb):
        return "date_trunc('%s', x)" % gb

    def search_partner(self, name):
        where = "WHERE name = '%s'" % name
        self.env.cr.execute("SELECT id FROM res_partner " + where)  # T2

    def message(self, name):
        raise ValueError(_("Select one from %s where x = 1") % name)

    def published(self, limit=None):
        limit_clause = '' if limit is None else ' LIMIT %d' % limit
        self.env.cr.execute("SELECT id FROM x WHERE id IN %s" + limit_clause, (tuple(self.ids),))

    def lines(self, data):
        clause = self._get_query_get_clause(data)
        self.env.cr.execute('SELECT id FROM ' + clause[0] + ' WHERE ' + clause[1], clause[2])

    def print_report(self, data):
        data = {}
        data['form'] = self.read()[0]
        self.env.cr.execute("SELECT 1 FROM p WHERE p.id IN %s" % str(data['form']['ids']))  # R1

    def migrate_to_alias(self, table, init_fct):
        init_fct(self.env.cr)
        self.env.cr.execute('ALTER TABLE %s ALTER COLUMN alias_id SET NOT NULL' % table)  # R2


class Fields(models.Model):
    _inherit = "ir.model.fields"

    def write(self, vals):
        self.env.cr.execute('ALTER TABLE x RENAME "%s"' % vals['name'])  # T3
        res = super(Fields, self).write(vals)
        self.env.cr.execute('ALTER TABLE x RENAME "%s"' % vals['name'])  # A1
        return res

    def rename(self, name):
        super().write({})
        self.env.cr.execute('ALTER TABLE x RENAME "%s"' % name)  # T4


def normalize(src):
    src = DOCTYPE.sub("", src)  # H1
    src = re.sub(r'</?o:.*?>', '', src)  # H2
    src = re.sub(r'</?o:[^<>\n]*?>', '', src)
    text = re.sub(r'<[^>]+>', '', src)
    return re.sub(r'<.*?>', '', text)
"#,
        )
        .unwrap();
        let result = OGHCollectorAnalyzer::new(&110u8).analyze_module_source(&dir);
        fs::remove_dir_all(&dir).unwrap();
        let mut found: Vec<(String, String)> = result
            .security_facts
            .iter()
            .map(|f| {
                let detail = f.detail.as_deref().unwrap_or("");
                let marker = detail.rsplit("# ").next().unwrap_or("").to_string();
                (marker, f.kind.clone())
            })
            .collect();
        found.sort();
        let expected = [
            ("A1", "sql_injection_admin"),
            ("H1", "regex_html_filter"),
            ("H2", "regex_html_filter"),
            ("R1", "sql_injection"),
            ("R2", "sql_injection"),
            ("T1", "sql_injection_tainted"),
            ("T2", "sql_injection_tainted"),
            ("T3", "sql_injection_tainted"),
            ("T4", "sql_injection_tainted"),
        ];
        assert_eq!(
            found,
            expected
                .iter()
                .map(|(m, k)| (m.to_string(), k.to_string()))
                .collect::<Vec<_>>()
        );
    }

    // Rule-calibration harness, not a regression test: runs the extractor and
    // every judging rule over real checkouts and prints per-code counts plus a
    // few samples, so a rule change can be measured against real Odoo/OCA
    // source before it reaches the public site. Usage:
    //   OGH_CALIBRATE="<addons dir>[:<addons dir>...]" OGH_CALIBRATE_VERSION=180 \
    //     cargo test -p collector calibrate -- --ignored --nocapture
    // Each dir is scanned two levels deep for __manifest__.py/__openerp__.py.
    #[test]
    #[ignore]
    fn calibrate() {
        // Surfaces "Module source analysis failed" (RUST_LOG=warn): a crash
        // must not pass for a clean module.
        let _ = env_logger::builder().is_test(true).try_init();
        let roots = std::env::var("OGH_CALIBRATE").expect("set OGH_CALIBRATE");
        let version: u8 = std::env::var("OGH_CALIBRATE_VERSION")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(180);
        let analyzer = OGHCollectorAnalyzer::new(&version);
        let mut module_dirs = Vec::new();
        for root in roots.split(':') {
            let mut stack = vec![(PathBuf::from(root), 0)];
            while let Some((dir, depth)) = stack.pop() {
                if analyzer
                    .is_odoo_module_folder(&dir)
                    .ok()
                    .flatten()
                    .is_some()
                {
                    module_dirs.push(dir);
                } else if depth < 2 {
                    for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
                        if e.path().is_dir() {
                            stack.push((e.path(), depth + 1));
                        }
                    }
                }
            }
        }
        let mut by_code: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        let mut empty = 0;
        for dir in &module_dirs {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            let manifest = ["__manifest__.py", "__openerp__.py"]
                .iter()
                .map(|f| dir.join(f))
                .find(|p| p.is_file())
                .and_then(|p| {
                    analyzer
                        .read_manifest("o", "r", &name, &p.to_string_lossy())
                        .ok()
                });
            let category = manifest.map(|m| m.category).unwrap_or_default();
            let a = analyzer.analyze_module_source(dir);
            if a.views.is_empty()
                && a.models.is_empty()
                && a.records.is_empty()
                && a.controllers.is_empty()
            {
                empty += 1;
            }
            let mut found: Vec<(String, String, String)> = Vec::new();
            for w in crate::security::analyze_module(&name, &category, &a) {
                found.push((
                    format!("sec {} {}", w.severity, w.code),
                    w.xml_id.unwrap_or_default(),
                    format!("{}:{}", w.file.unwrap_or_default(), w.line.unwrap_or(0)),
                ));
            }
            for n in crate::migration::analyze_module(version, &a) {
                found.push((
                    format!("mig {} {}", n.severity, n.code),
                    n.context.unwrap_or_default(),
                    format!(
                        "{}:{} {}",
                        n.file.unwrap_or_default(),
                        n.line.unwrap_or(0),
                        n.message
                    ),
                ));
            }
            for (code, ctx, loc) in found {
                by_code
                    .entry(code)
                    .or_default()
                    .push(format!("{} {ctx} {loc}", dir.display()));
            }
        }
        println!(
            "modules scanned: {} (empty analysis: {empty})",
            module_dirs.len()
        );
        let samples: usize = std::env::var("OGH_CALIBRATE_SAMPLES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(4);
        for (code, hits) in &by_code {
            println!("{:>6}  {code}", hits.len());
            for h in hits
                .iter()
                .step_by((hits.len() / samples.max(1)).max(1))
                .take(samples)
            {
                println!("          {}", h.chars().take(260).collect::<String>());
            }
        }
    }

    // Proves the incremental-analysis short-circuit end to end: a module
    // analyzed once and persisted (like main.rs does after every run) must
    // be skipped on a re-run with no new commits, and picked back up as soon
    // as a commit actually touches it.
}
