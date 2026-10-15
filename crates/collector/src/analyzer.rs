// Copyright Alexandre D. Díaz
//! `OGHCollectorAnalyzer`: walks cloned repo directories, uses **PyO3** to invoke Python to parse
//! Odoo `__manifest__.py` files, extracts `ManifestInfo`.
//!
//! Also embeds the Python extractor (`ANALYZER_PY_SRC`, stdlib-only, Python 3.8-compatible) that
//! emits raw `migration_facts`/`security_facts` (no judging) plus views/models/records/controllers.
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
# Names a model method can't receive from an RPC caller.
NON_RPC_PARAMS = {"self", "cls", "cr", "registry", "env"}
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
    # (self._table, self._select()) are the standard reporting-model pattern.
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
    return isinstance(node, ast.Attribute) and node.attr.startswith("_")


def _sql_risky_values(node):
    # The unsafe values a %, +, .format() or f-string expression splices into
    # a SQL string; [] when it isn't string-building or everything is safe.
    if isinstance(node, ast.BinOp) and isinstance(node.op, ast.Mod):
        right = node.right
        if isinstance(right, (ast.Tuple, ast.List)):
            values = right.elts
        elif isinstance(right, ast.Dict):
            values = right.values
        else:
            values = [right]
        return [v for v in values if not _sql_value_safe(v)] + _sql_risky_values(node.left)
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


def _is_request_payload(node):
    # The raw client payload of an HTTP request.
    r = _expr_repr(node) or ""
    return r.startswith(("request.params", "request.httprequest.form", "request.httprequest.args"))


def _caller_names(node):
    # Names whose own value flows into `node`. `rec.amount` reads a stored
    # field (not the caller's text), so a Name used only as an attribute base
    # doesn't count - a method call on it (`kw.get('x')`) still does.
    calls = {id(n.func) for n in ast.walk(node) if isinstance(n, ast.Call)}
    bases = {
        id(n.value) for n in ast.walk(node) if isinstance(n, ast.Attribute) and id(n) not in calls
    }
    return {n.id for n in ast.walk(node) if isinstance(n, ast.Name) and id(n) not in bases}


def _is_client_controlled(node, params):
    # Values an RPC/HTTP caller sets directly: the method's own parameters
    # (any public model method is callable over RPC), the request payload and
    # the context (also sent by the client).
    if _caller_names(node) & params:
        return True
    for n in ast.walk(node):
        r = _expr_repr(n) or ""
        if _is_request_payload(n) or r.startswith(
            ("self._context", "self.env.context", "request.httprequest", "request.get_json_data", "request.jsonrequest")
        ):
            return True
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


def _analyze_sql_injection(tree, rel_path, lines):
    facts = []
    for owner in ast.walk(tree):
        if not isinstance(owner, ast.ClassDef):
            continue
        for func in owner.body:
            if not isinstance(func, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            # Only parameters of an RPC/route-reachable method count as
            # client-controlled; @api.private (17.0+) opts a method out of RPC.
            exposed = (
                _is_model_class(owner)
                and not func.name.startswith("_")
                and not any((_expr_repr(d) or "").endswith("private") for d in func.decorator_list)
            )
            params = _function_params(func) if exposed or _route_decorator(func) else set()
            # `assert mode in (...)` / `if mode not in ALLOWED: raise` is an
            # allow-list check: that parameter can't carry arbitrary SQL.
            params -= {
                n.left.id
                for n in ast.walk(func)
                if isinstance(n, ast.Compare)
                and isinstance(n.left, ast.Name)
                and any(isinstance(op, (ast.In, ast.NotIn)) for op in n.ops)
            }
            assigned = {}
            for n in ast.walk(func):
                if isinstance(n, ast.Assign):
                    for t in n.targets:
                        if isinstance(t, ast.Name):
                            assigned.setdefault(t.id, []).append(n.value)
                elif isinstance(n, ast.AugAssign) and isinstance(n.target, ast.Name):
                    assigned.setdefault(n.target.id, []).append(n.value)
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
                if not (is_execute or is_sql) or _suppressed(lines, n, func):
                    continue
                query = n.args[0]
                risky = [p for v in _sql_risky_values(query) for p in _risky_parts(v)]
                if not risky and isinstance(query, ast.Name):
                    # One hop: the query string built a few lines above.
                    for value in assigned.get(query.id, []):
                        if _suppressed(lines, value):
                            continue
                        risky += [p for v in _sql_risky_values(value) for p in _risky_parts(v)] or (
                            [value]
                            if isinstance(value, ast.Name) and value.id in params
                            else []
                        )
                if not risky:
                    continue
                # Also one hop for each spliced value (`where = kw['x']`).
                tainted = any(
                    _is_client_controlled(v, params)
                    or any(
                        _is_client_controlled(src, params)
                        for name in _caller_names(v)
                        for src in assigned.get(name, [])
                    )
                    for v in risky
                )
                facts.append(
                    {
                        "kind": "sql_injection_tainted" if tainted else "sql_injection",
                        "context": f"{owner.name}.{func.name}",
                        "detail": _truncate(lines[n.lineno - 1].strip()) if n.lineno <= len(lines) else None,
                        "file": rel_path,
                        "line": n.lineno,
                    }
                )
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
    # @route(...) / @http.route(...) / @odoo.http.route(...); a bare @route
    # (no call) is a pure override of an inherited route and also counts.
    for dec in func_node.decorator_list:
        target = dec.func if isinstance(dec, ast.Call) else dec
        name = (
            target.attr
            if isinstance(target, ast.Attribute)
            else (target.id if isinstance(target, ast.Name) else None)
        )
        if name == "route":
            return dec
    return None


def _calls_method(func_node, method_name):
    for n in ast.walk(func_node):
        if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute) and n.func.attr == method_name:
            return True
    return False


def _analyze_controllers(tree, rel_path):
    # Every HTTP endpoint a module exposes: any method decorated with
    # http.route, whatever the class inherits from (matching on the decorator
    # instead of the Controller base also catches classes extending existing
    # controllers, e.g. `class Extended(WebsiteSale)`).
    out = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.ClassDef):
            continue
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
                    "uses_sudo": _calls_method(stmt, "sudo"),
                    # Portal token-validation pattern: the endpoint gates its
                    # record access on _document_check_access(..., access_token).
                    "checks_token_access": _calls_method(stmt, "_document_check_access"),
                    "mass_assignment": _safe(_mass_assignment, stmt, default=False),
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

/// A manifest parsed with `ast.literal_eval`, like Odoo's own loader: nothing
/// in it runs. For PR heads, which come from anyone's fork (`read_manifest`
/// evals the merged code's manifests).
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
pub const ANALYSIS_VERSION: i32 = 1;

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
            let manifest = py.eval(&code, None, None)?.cast_into::<PyDict>()?;
            // name
            let name_opt = manifest.get_item("name")?;
            let name: String = if let Some(name_value) = name_opt {
                name_value.cast::<PyString>()?.extract::<String>()?
            } else {
                String::new()
            };
            // description - readme/DESCRIPTION.md (OCA's rendered readme fragment) wins
            // over the manifest's `description` key when present, since the manifest
            // value is often stale or just a placeholder for the generated readme.
            let description_opt = manifest.get_item("description")?;
            let description: String = if let Some(description_value) = description_opt {
                description_value.cast::<PyString>()?.extract::<String>()?
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
            // static/description/icon.png, base64-encoded so it can travel through
            // the same text column/JSON path as description/installation/usage; the
            // module page falls back to a generic icon when this is empty.
            let icon = module_dir.and_then(Self::read_icon).unwrap_or_default();
            // author
            let author_opt = manifest.get_item("author")?;
            let author: String = if let Some(author_value) = author_opt {
                match author_value.cast::<PyString>() {
                    Ok(pyval) => pyval.extract::<String>()?,
                    Err(_) => match author_value.cast::<PyList>() {
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
            let website_opt = manifest.get_item("website")?;
            let website: String = if let Some(website_value) = website_opt {
                website_value.cast::<PyString>()?.extract::<String>()?
            } else {
                String::new()
            };
            // license
            let license_opt = manifest.get_item("license")?;
            let license: String = if let Some(license_value) = license_opt {
                license_value.cast::<PyString>()?.extract::<String>()?
            } else if self.version_odoo < 90 {
                // OpenERP <= 8.0 defaulted a missing manifest license to
                // AGPL-3; Odoo 9.0 switched the default (and core) to LGPL-3.
                "AGPL-3".to_string()
            } else {
                "LGPL-3".to_string()
            };
            // category
            let category_opt = manifest.get_item("category")?;
            let category: String = if let Some(category_value) = category_opt {
                category_value.cast::<PyString>()?.extract::<String>()?
            } else {
                "Uncategorized".to_string()
            };
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
            let version_opt = manifest.get_item("version")?;
            let version_odoo: u8;
            let version_module: String = if let Some(version_value) = version_opt {
                let version = version_value.cast::<PyString>()?.extract::<String>()?;
                let odoo_ver = OdooVersion::new(&version, &self.version_odoo);
                version_odoo = *odoo_ver.get_version_odoo();
                odoo_ver.get_version_module().clone()
            } else {
                version_odoo = self.version_odoo;
                "0.1.0".to_string()
            };
            // application
            let application_opt = manifest.get_item("application")?;
            let application: bool = if let Some(application_value) = application_opt {
                application_value.cast::<PyBool>()?.extract::<bool>()?
            } else {
                false
            };
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
            let maintainer_opt = manifest.get_item("maintainer")?;
            let maintainer: String = if let Some(maintainer_value) = maintainer_opt {
                match maintainer_value.cast::<PyString>() {
                    Ok(pyval) => pyval.extract::<String>()?,
                    Err(_) => match maintainer_value.cast::<PyList>() {
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
            let depends_opt = manifest.get_item("depends")?;
            let depends: Vec<String> = if let Some(depends_value) = depends_opt {
                depends_value.cast::<PyList>()?.extract::<Vec<String>>()?
            } else {
                Vec::new()
            };
            let mut external_depends_python_set: HashSet<String> = HashSet::new();
            let mut external_depends_bin_set: HashSet<String> = HashSet::new();
            if let Some(external_depends_value) = manifest.get_item("external_dependencies")? {
                let depends_dict = external_depends_value.cast::<PyDict>()?;
                // "deb" is an unofficial way to get the "debian" package name (used by OCA CI)
                for key in ["python", "bin", "deb"] {
                    let Some(deps_value) = depends_dict.get_item(key)? else {
                        continue;
                    };
                    let Ok(deps_list) = deps_value.cast::<PyList>() else {
                        continue;
                    };
                    let target = if key == "python" {
                        &mut external_depends_python_set
                    } else {
                        &mut external_depends_bin_set
                    };
                    for dep_name in deps_list.iter() {
                        target.insert(dep_name.extract()?);
                    }
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


class Ctl(http.Controller):
    @http.route("/x/update", auth="public", csrf=False, methods=["POST"])
    def update(self, **kw):
        request.env["x.thing"].sudo().browse(1).write(kw)

    @http.route("/x/safe", auth="user")
    def safe(self, **kw):
        kw = {k: kw[k] for k in ("name",) if k in kw}
        request.env["x.thing"].browse(1).write(kw)

    @http.route("/x/find", auth="public")
    def find(self, q=None):
        request.env.cr.execute("SELECT 1 WHERE n = '%s'" % q)  # T4


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
            ("R1", "sql_injection"),
            ("R2", "sql_injection"),
            ("R3", "sql_injection"),
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
        // Reassigned (filtered) kwargs before write: not mass assignment.
        let safe = result
            .controllers
            .iter()
            .find(|c| c.name == "safe")
            .unwrap();
        assert!(!safe.mass_assignment);
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
