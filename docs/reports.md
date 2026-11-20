# Reports, license review, feeds and badges

- Static findings are review priorities, not confirmed vulnerabilities. "Grave" (`error`) is
  reserved for high-precision cases: SQL built by string formatting from caller-controlled input
  (an RPC-callable method's parameters, the request payload or the context; CWE-89), routes
  writing the raw request payload into `write()`/`create()` without access checks (CWE-915),
  write/unlink ACLs open to every user or to portal/public, always-true portal/public record
  rules, CSRF disabled on authenticated state-changing routes, and write access to security
  models. Other SQL formatting, `eval`/`exec`, pickle/unsafe YAML, `shell=True`,
  `verify=False` and user-writable `Html(sanitize=False)` fields are minor review items.
- Noise control: `test_*` / "Tests"-category modules (never installed in production) are not
  scanned, intentional broad-access naming (`public`, `all`, portal ACLs named after their
  group) and create-only grants are not grave, an always-true portal rule over the module's own
  deny-all ACL is ignored, and the markers OCA uses for reviewed hits
  (`# pylint: disable=sql-injection`, `# noqa`, `# nosec`) are honored. MCP
  `get_module_code_analysis` includes `security_warnings` and migration notes with source
  file/line where available.
- Migration notes know which Odoo version each API change happened in (checked against Odoo's
  source): "Migrating to X.0: ..." for a module still below it, "Since X.0: ..." when the
  module's own version already ignores or rejects that code. Repeats of one note within a file
  are folded into one line.
- Live scans return `findings`, endpoint evidence and `checks` (`passed: null` means inconclusive).
  The arbitrary numeric `score` has been removed from the JSON response and dashboard. HTTP 200
  alone does not establish sensitive-file or database exposure, and module names are not used to
  guess a running Odoo version.

Each module stores the `ANALYSIS_VERSION` (`crates/collector/src/analyzer.rs`) its findings were
computed with (and which analyses ran, see [Workflows](workflows.md)); bump it whenever the extractor
or the security/migration rules change, and the next collector run re-analyzes every module, even
those whose branch gets no new commits. To force a
full re-analysis regardless, set `OGHCOLLECTOR_FORCE_REANALYZE=1`:

```sh
OGHCOLLECTOR_FORCE_REANALYZE=1 cargo run --bin collector -- OCA 18.0
```

## License review, feeds, badges and version comparison

- **License review**: every module page, the Doodba dependency resolver, shared packs, MCP
  (`get_module_dependencies`, `review_module_licenses`) and `GET /v1/license-check` check the
  manifest licenses of the modules *and their whole dependency closure*: the license the combined
  work must be distributed under (e.g. an LGPL-3 module on an AGPL-3 dependency is effectively
  AGPL-3), proprietary modules pulled in, and conflicts (proprietary code on GPL/AGPL, GPL-2-only
  mixed with v3). It's a heuristic to catch problems early, not legal advice. In CI:

  ```sh
  curl -s "https://ommd.example/v1/license-check?odoo_version=18.0&modules=sale_order_type,web_responsive" \
    | jq -e '[.findings[] | select(.severity == "error")] | length == 0'
  ```

- **Atom feeds**: `/feed.atom` (new/updated modules, migration PRs, vulnerabilities; filter with
  `type`, `odoo_version`, `q`) and `/module/{org}/{technical_name}/feed.atom` to follow one
  module. Works with any feed reader or Slack/Teams RSS integration.
- **README badges**: `/badge/{org}/{technical_name}/{versions|status|license|security}.svg`; module
  pages have copy-ready Markdown/reStructuredText/HTML snippets under "Embed badges".
- **Version comparison**: "Compare" on a module page (or `GET /v1/compare/{module}?from=&to=`, MCP
  `compare_module_versions`) shows what changed between two Odoo versions: manifest, dependencies,
  models/fields/methods, views and HTTP routes.
