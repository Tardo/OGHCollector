// Copyright Alexandre D. Díaz
//! Workflow definitions and their rules: what `collector.yaml` `workflows:`
//! may say. Shared by the collector (which runs them) and the server (whose
//! editor builds and validates them), so both apply the same rules.
use std::collections::{HashMap, HashSet};

use minijinja::Environment;
use serde::{Deserialize, Serialize};

const BUILT_IN: &str = include_str!("default_workflows.yaml");
/// What a source without `workflows:` runs.
pub const DEFAULT: &str = "default";

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    /// What runs it: `module` (each module folder a job walks) or
    /// `pull_request` (each PR/MR it tracks).
    pub on: String,
    /// The first node is where it starts.
    pub nodes: Vec<Node>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    /// Node to run after this one (none: the workflow ends here).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// `if`: the condition and the node to go to when it's true / false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub then: Option<String>,
    #[serde(rename = "else", skip_serializing_if = "Option::is_none")]
    pub otherwise: Option<String>,
    /// `log`: event type, severity and message template of the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// `ai`: `openai` (any compatible API) or `anthropic`, the model, prompt
    /// and system templates, base URL, env var (or Docker secret) holding the
    /// key, answer size and seconds to wait.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// `pr_comment`/`pr_edit`/`pr_open`: templates of the comment or
    /// description, the PR title and the patch it commits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub patch: Option<String>,
}

impl Node {
    fn fields(&self) -> impl Iterator<Item = &'static str> + '_ {
        [
            ("next", self.next.is_some()),
            ("when", self.when.is_some()),
            ("then", self.then.is_some()),
            ("else", self.otherwise.is_some()),
            ("event", self.event.is_some()),
            ("severity", self.severity.is_some()),
            ("message", self.message.is_some()),
            ("provider", self.provider.is_some()),
            ("model", self.model.is_some()),
            ("prompt", self.prompt.is_some()),
            ("system", self.system.is_some()),
            ("url", self.url.is_some()),
            ("api_key_env", self.api_key_env.is_some()),
            ("max_tokens", self.max_tokens.is_some()),
            ("timeout", self.timeout.is_some()),
            ("body", self.body.is_some()),
            ("title", self.title.is_some()),
            ("patch", self.patch.is_some()),
        ]
        .into_iter()
        .filter_map(|(field, set)| set.then_some(field))
    }

    fn targets(&self) -> impl Iterator<Item = &str> {
        [&self.next, &self.then, &self.otherwise]
            .into_iter()
            .filter_map(|t| t.as_deref())
    }
}

const MODULE: &[&str] = &["module"];
const PULL_REQUEST: &[&str] = &["pull_request"];
const ANY: &[&str] = &["module", "pull_request"];
const AI_FIELDS: &[&str] = &[
    "next",
    "provider",
    "model",
    "prompt",
    "system",
    "url",
    "api_key_env",
    "max_tokens",
    "timeout",
];

/// Node types, the fields each takes besides `id`/`type`, and the triggers
/// it can run on.
const NODE_TYPES: [(&str, &[&str], &[&str]); 13] = [
    ("if", &["when", "then", "else"], ANY),
    ("read_manifest", &["next"], MODULE),
    ("git_stats", &["next"], MODULE),
    ("analyze_code", &["next"], MODULE),
    ("security", &["next"], MODULE),
    ("migration_notes", &["next"], MODULE),
    ("store_module", &["next"], MODULE),
    ("osv", &["next"], MODULE),
    ("log", &["next", "event", "severity", "message"], ANY),
    ("ai", AI_FIELDS, ANY),
    ("pr_comment", &["next", "body"], PULL_REQUEST),
    ("pr_edit", &["next", "body"], PULL_REQUEST),
    ("pr_open", &["next", "title", "body", "patch"], MODULE),
];

/// The outward node types a source can let act for real (`actions`).
pub const ACTIONS: [&str; 4] = ["ai", "pr_comment", "pr_edit", "pr_open"];
const PROVIDERS: [&str; 2] = ["openai", "anthropic"];
/// `ai.timeout` at most: with `OGHCOLLECTOR_AI_LIMIT` calls per job it must
/// stay well within the job timeout.
const MAX_AI_TIMEOUT: u64 = 600;

const SEVERITIES: [&str; 4] = ["info", "success", "warning", "error"];

/// What each node type does, for people (the editor's palette).
const NODE_DOCS: [(&str, &str); 13] = [
    (
        "if",
        "Goes to `then` when the condition holds, else to `else`",
    ),
    ("read_manifest", "Reads the module's manifest"),
    ("git_stats", "Committers and folder size, from git"),
    ("analyze_code", "Analyzes the .py/.xml/.csv files"),
    ("security", "Security rules over the analysis"),
    (
        "migration_notes",
        "Migration notes for the collected version",
    ),
    (
        "store_module",
        "Stores the module and what the workflow computed",
    ),
    (
        "osv",
        "Known vulnerabilities of the pinned Python dependencies just added",
    ),
    ("log", "An event in /logs"),
    ("ai", "Asks an AI; the answer is ai.<node id> (outward)"),
    ("pr_comment", "Comments on the PR/MR (outward)"),
    ("pr_edit", "Replaces the PR/MR description (outward)"),
    (
        "pr_open",
        "Opens a PR/MR with a patch on the module (outward)",
    ),
];

/// How each field is edited (`edge`: a link to another node; `condition`:
/// a Jinja expression; `template`: Jinja text) and what it's for.
const FIELDS: [(&str, &str, &str); 18] = [
    ("next", "edge", "Node to run after this one"),
    (
        "when",
        "condition",
        "Jinja condition: true goes to `then`, false to `else`",
    ),
    ("then", "edge", "Node when the condition holds"),
    (
        "else",
        "edge",
        "Node when it doesn't (none: the workflow ends)",
    ),
    ("event", "text", "Event type in /logs (default: workflow)"),
    ("severity", "enum", "Event severity (default: info)"),
    ("message", "template", "Event text"),
    (
        "provider",
        "enum",
        "openai (any compatible API: Ollama, LM Studio, vLLM...) or anthropic",
    ),
    ("model", "text", "Model name"),
    ("prompt", "template", "What to ask"),
    ("system", "template", "System prompt"),
    ("url", "text", "Base URL (default: the provider's)"),
    (
        "api_key_env",
        "text",
        "Env var (or Docker secret) holding the API key",
    ),
    (
        "max_tokens",
        "number",
        "Answer size at most (default: 16000)",
    ),
    (
        "timeout",
        "number",
        "Seconds to wait, 1 to 600 (default: 300)",
    ),
    ("body", "template", "Comment or description text"),
    ("title", "template", "PR/MR title"),
    (
        "patch",
        "template",
        "Unified diff to commit, e.g. {{ ai.fix }}",
    ),
];

/// The fields `validate` requires, per node type.
const REQUIRED: [(&str, &[&str]); 6] = [
    ("if", &["when", "then"]),
    ("log", &["message"]),
    ("ai", &["provider", "model", "prompt"]),
    ("pr_comment", &["body"]),
    ("pr_edit", &["body"]),
    ("pr_open", &["title", "patch"]),
];

/// Everything an editor needs to build valid workflows, from the same
/// tables `validate` uses.
pub fn metadata() -> serde_json::Value {
    let node_types: Vec<serde_json::Value> = NODE_TYPES
        .iter()
        .map(|(kind, fields, triggers)| {
            let doc = NODE_DOCS
                .iter()
                .find(|(k, _)| k == kind)
                .map_or("", |(_, d)| d);
            let required = REQUIRED
                .iter()
                .find(|(k, _)| k == kind)
                .map_or(&[][..], |(_, r)| r);
            let outputs: Vec<&str> = fields
                .iter()
                .copied()
                .filter(|f| FIELDS.iter().any(|(n, k, _)| n == f && *k == "edge"))
                .collect();
            serde_json::json!({
                "type": kind,
                "description": doc,
                "fields": fields,
                "required": required,
                "triggers": triggers,
                "outputs": outputs,
            })
        })
        .collect();
    let fields: serde_json::Map<String, serde_json::Value> = FIELDS
        .iter()
        .map(|(name, kind, doc)| {
            let values: &[&str] = match *name {
                "provider" => &PROVIDERS,
                "severity" => &SEVERITIES,
                _ => &[],
            };
            let spec = serde_json::json!({"kind": kind, "description": doc, "values": values});
            (name.to_string(), spec)
        })
        .collect();
    serde_json::json!({
        "triggers": ANY,
        "actions": ACTIONS,
        "node_types": node_types,
        "fields": fields,
        "variables": {
            "module": [
                "technical_name", "path", "source_path", "org", "repo", "odoo_version", "is_new",
                "unchanged", "last_commit.hash", "last_commit.author", "last_commit.date",
                "last_commit.name", "manifest.name", "manifest.version", "manifest.license",
                "manifest.category", "manifest.author", "manifest.installable", "manifest.depends",
                "security.grave", "security.minor", "security.findings", "migration.count",
                "migration.notes", "ai.<node id>", "file('rel/path')",
            ],
            "pull_request": [
                "org", "repo", "odoo_version", "pr.number", "pr.title", "pr.author", "pr.labels",
                "pr.draft", "pr.ci_status", "pr.head_sha", "pr.head_ref", "pr.url",
                "pr.description", "pr.is_new", "pr.head_changed", "pr.ci_changed", "pr.is_ours",
                "modules[].name", "modules[].port", "modules[].analysis", "ai.<node id>", "diff()",
            ],
        },
    })
}

/// Rejects what a YAML document from an untrusted sender could use to make
/// parsing blow up: aliases (a few KB of nested `&a [*b, *b, ...]` expand to
/// billions of nodes, and the allocation failure aborts the process) and
/// deep nesting. Reading the events expands nothing, so this is cheap. The
/// editor never sends either; the collector config (trusted) skips this.
pub fn check_untrusted_yaml(yaml: &str) -> Result<(), String> {
    use yaml_rust2::parser::{Event, Parser};
    const MAX_DEPTH: usize = 64;
    let mut parser = Parser::new_from_str(yaml);
    let mut depth = 0usize;
    loop {
        let (event, _) = parser.next_token().map_err(|err| err.to_string())?;
        match event {
            Event::StreamEnd => return Ok(()),
            Event::Alias(_) => return Err("YAML aliases (*name) aren't accepted here".to_string()),
            Event::SequenceStart(..) | Event::MappingStart(..) => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err(format!("nested deeper than {MAX_DEPTH} levels"));
                }
            }
            Event::SequenceEnd | Event::MappingEnd => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
}

/// The `workflows:` of a YAML document (a whole `collector.yaml` works: the
/// rest is ignored), read the way the collector reads it.
pub fn parse_workflows(yaml: &str) -> Result<HashMap<String, Workflow>, String> {
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        workflows: HashMap<String, Workflow>,
    }
    config::Config::builder()
        .add_source(config::File::from_str(yaml, config::FileFormat::Yaml))
        .build()
        .and_then(|c| c.try_deserialize::<Doc>())
        .map(|doc| doc.workflows)
        .map_err(|err| err.to_string())
}

/// The `workflows:` of a collector config file (`config` crate: `path` with
/// or without its .yaml/.json/.toml extension). No file: none.
pub fn file_workflows(path: &str) -> Result<HashMap<String, Workflow>, String> {
    #[derive(Deserialize)]
    struct Doc {
        #[serde(default)]
        workflows: HashMap<String, Workflow>,
    }
    config::Config::builder()
        .add_source(config::File::with_name(path).required(false))
        .build()
        .and_then(|c| c.try_deserialize::<Doc>())
        .map(|doc| doc.workflows)
        .map_err(|err| format!("Can't load the workflows of '{path}': {err}"))
}

/// Workflow names, node ids and event types: lowercase (the `config` crate
/// lowercases map keys), digits, `-` and `_`.
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// The collector's own secrets (forge tokens, webhook secrets), by env var or
/// Docker secret name.
fn is_collector_secret(name: &str) -> bool {
    let name = name.to_lowercase();
    name.starts_with("oghcollector_")
        || ["gh", "gl"].iter().any(|forge| {
            ["token", "write_token", "webhook_secret"]
                .iter()
                .any(|kind| name == format!("{forge}_{kind}"))
        })
}

/// Everything a workflow can get wrong, checked before any job runs.
pub fn validate(name: &str, wf: &Workflow) -> Result<(), String> {
    let fail = |msg: String| Err(format!("workflow '{name}': {msg}"));
    if !is_valid_name(name) {
        return fail("invalid name (lowercase letters, digits, '-' or '_')".to_string());
    }
    if !ANY.contains(&wf.on.as_str()) {
        return fail(format!(
            "unknown trigger '{}' (module or pull_request)",
            wf.on
        ));
    }
    if wf.nodes.is_empty() {
        return fail("no nodes".to_string());
    }
    let mut ids = HashSet::new();
    for node in &wf.nodes {
        if !is_valid_name(&node.id) || !ids.insert(node.id.as_str()) {
            return fail(format!("invalid or repeated node id '{}'", node.id));
        }
    }
    let env = Environment::new();
    for node in &wf.nodes {
        let at = format!("node '{}'", node.id);
        let Some((_, allowed, triggers)) = NODE_TYPES.iter().find(|(kind, ..)| *kind == node.kind)
        else {
            let kinds: Vec<&str> = NODE_TYPES.iter().map(|(kind, ..)| *kind).collect();
            return fail(format!(
                "{at}: unknown type '{}' ({})",
                node.kind,
                kinds.join(", ")
            ));
        };
        if !triggers.contains(&wf.on.as_str()) {
            return fail(format!("{at}: '{}' can't run `on: {}`", node.kind, wf.on));
        }
        if let Some(field) = node.fields().find(|f| !allowed.contains(f)) {
            return fail(format!("{at}: '{field}' isn't a field of '{}'", node.kind));
        }
        let template = |field: &str, src: &Option<String>, required: bool| match src {
            None if required => Err(format!("{at}: `{}` needs `{field}`", node.kind)),
            None => Ok(()),
            Some(src) => env
                .template_from_str(src)
                .map(drop)
                .map_err(|err| format!("{at}: bad {field}: {err}")),
        };
        if let Some(target) = node.targets().find(|t| !ids.contains(t)) {
            return fail(format!("{at}: unknown node '{target}'"));
        }
        match node.kind.as_str() {
            "if" => {
                let (Some(when), Some(_)) = (&node.when, &node.then) else {
                    return fail(format!("{at}: `if` needs `when` and `then`"));
                };
                if let Err(err) = env.compile_expression(when) {
                    return fail(format!("{at}: bad condition: {err}"));
                }
            }
            "log" => {
                let Some(message) = &node.message else {
                    return fail(format!("{at}: `log` needs a `message`"));
                };
                if let Err(err) = env.template_from_str(message) {
                    return fail(format!("{at}: bad message: {err}"));
                }
                if node
                    .severity
                    .as_deref()
                    .is_some_and(|s| !SEVERITIES.contains(&s))
                {
                    return fail(format!(
                        "{at}: severity is one of {}",
                        SEVERITIES.join(", ")
                    ));
                }
                if node.event.as_deref().is_some_and(|e| !is_valid_name(e)) {
                    return fail(format!("{at}: invalid event type"));
                }
            }
            "ai" => {
                if !node
                    .provider
                    .as_deref()
                    .is_some_and(|p| PROVIDERS.contains(&p))
                {
                    return fail(format!("{at}: provider is one of {}", PROVIDERS.join(", ")));
                }
                if node.model.is_none() {
                    return fail(format!("{at}: `ai` needs `model`"));
                }
                if node.timeout.is_some_and(|t| t == 0 || t > MAX_AI_TIMEOUT) {
                    return fail(format!("{at}: timeout is 1 to {MAX_AI_TIMEOUT} seconds"));
                }
                if node.max_tokens == Some(0) {
                    return fail(format!("{at}: max_tokens must be positive"));
                }
                // With any `url`, that would send the forge's tokens away.
                if node.api_key_env.as_deref().is_some_and(is_collector_secret) {
                    return fail(format!(
                        "{at}: api_key_env can't be one of the collector's own secrets"
                    ));
                }
                template("prompt", &node.prompt, true).or_else(fail)?;
                template("system", &node.system, false).or_else(fail)?;
            }
            "pr_comment" | "pr_edit" => template("body", &node.body, true).or_else(fail)?,
            "pr_open" => {
                template("title", &node.title, true).or_else(fail)?;
                template("patch", &node.patch, true).or_else(fail)?;
                template("body", &node.body, false).or_else(fail)?;
            }
            _ => {}
        }
    }
    // A cycle would run forever: every path must end.
    fn visit<'a>(
        wf: &'a Workflow,
        id: &'a str,
        path: &mut Vec<&'a str>,
        done: &mut HashSet<&'a str>,
    ) -> Result<(), String> {
        if path.contains(&id) {
            return Err(format!("cycle through node '{id}'"));
        }
        if !done.insert(id) {
            return Ok(());
        }
        path.push(id);
        let node = wf
            .nodes
            .iter()
            .find(|n| n.id == id)
            .expect("targets were checked");
        for target in node.targets() {
            visit(wf, target, path, done)?;
        }
        path.pop();
        Ok(())
    }
    let mut done = HashSet::new();
    for node in &wf.nodes {
        if let Err(err) = visit(wf, &node.id, &mut Vec::new(), &mut done) {
            return fail(err);
        }
    }
    Ok(())
}

/// The built-in workflows, then the config file's (same name: the file's).
pub fn with_built_in(file: &HashMap<String, Workflow>) -> HashMap<String, Workflow> {
    let mut all: HashMap<String, Workflow> = config::Config::builder()
        .add_source(config::File::from_str(BUILT_IN, config::FileFormat::Yaml))
        .build()
        .and_then(|c| c.try_deserialize())
        .expect("the built-in workflows are valid");
    all.extend(file.iter().map(|(name, wf)| (name.clone(), wf.clone())));
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> Workflow {
        config::Config::builder()
            .add_source(config::File::from_str(yaml, config::FileFormat::Yaml))
            .build()
            .and_then(|c| c.try_deserialize())
            .unwrap()
    }

    #[test]
    fn the_editor_metadata_covers_every_rule() {
        for (kind, fields, _) in NODE_TYPES {
            assert!(
                NODE_DOCS.iter().any(|(k, _)| *k == kind),
                "no description for {kind}"
            );
            for field in fields {
                assert!(
                    FIELDS.iter().any(|(f, ..)| f == field),
                    "no spec for {kind}.{field}"
                );
            }
        }
        for (kind, required) in REQUIRED {
            let (_, fields, _) = NODE_TYPES.iter().find(|(k, ..)| *k == kind).unwrap();
            assert!(required.iter().all(|r| fields.contains(r)), "{kind}");
        }
        let meta = metadata();
        let if_node = &meta["node_types"][0];
        assert_eq!(
            (&if_node["type"], &if_node["outputs"]),
            (
                &serde_json::json!("if"),
                &serde_json::json!(["then", "else"])
            )
        );
        assert_eq!(
            meta["fields"]["provider"]["values"],
            serde_json::json!(PROVIDERS)
        );
    }

    #[test]
    fn hostile_yaml_is_turned_down_before_parsing() {
        let mut bomb = String::from("a0: &a0 [x, x, x, x, x, x, x, x, x, x]\n");
        for i in 1..9 {
            let refs = vec![format!("*a{}", i - 1); 10].join(", ");
            bomb += &format!("a{i}: &a{i} [{refs}]\n");
        }
        assert!(check_untrusted_yaml(&bomb).unwrap_err().contains("aliases"));
        assert!(check_untrusted_yaml(&"[".repeat(200_000)).is_err());
        assert!(check_untrusted_yaml(&format!("{}x{}", "[".repeat(65), "]".repeat(65))).is_err());
        let ok = "workflows:\n  w: {on: module, nodes: [{id: l, type: log, message: hi}]}\n";
        assert!(check_untrusted_yaml(ok).is_ok());
    }

    #[test]
    fn workflows_parse_like_the_collector_reads_them() {
        let yaml = "sources: [{source: A, versions: [18.0]}]\nworkflows:\n  w: {on: module, nodes: [{id: l, type: log, message: hi}]}\n";
        let parsed = parse_workflows(yaml).unwrap();
        assert_eq!(parsed["w"].nodes[0].message.as_deref(), Some("hi"));
        // Serialized for the editor without the unset fields.
        let node = serde_json::to_value(&parsed["w"].nodes[0]).unwrap();
        assert_eq!(
            node,
            serde_json::json!({"id": "l", "type": "log", "message": "hi"})
        );
        assert!(
            parse_workflows("workflows: {w: {on: module, nodes: [{id: l, typo: x}]}}").is_err()
        );
    }

    #[test]
    fn workflows_are_validated_up_front() {
        let built_in = with_built_in(&HashMap::new());
        assert!(validate(DEFAULT, &built_in[DEFAULT]).is_ok());
        let bad = [
            (
                "on: push\nnodes: [{id: a, type: log, message: x}]",
                "unknown trigger",
            ),
            (
                "on: module\nnodes: [{id: a, type: pr_comment, body: x}]",
                "can't run `on: module`",
            ),
            (
                "on: pull_request\nnodes: [{id: a, type: security}]",
                "can't run `on: pull_request`",
            ),
            (
                "on: module\nnodes: [{id: a, type: ai, provider: openai, prompt: x}]",
                "needs `model`",
            ),
            (
                "on: module\nnodes: [{id: a, type: ai, provider: gemini, model: m, prompt: x}]",
                "provider is one of",
            ),
            (
                "on: module\nnodes: [{id: a, type: ai, provider: openai, model: m, prompt: x, timeout: 3600}]",
                "timeout is 1 to",
            ),
            (
                "on: module\nnodes: [{id: a, type: pr_open, title: t}]",
                "needs `patch`",
            ),
            (
                "on: module\nnodes: [{id: a, type: ai, provider: openai, model: m, prompt: x, api_key_env: OGHCOLLECTOR_WRITE_TOKEN_GH}]",
                "collector's own secrets",
            ),
            (
                "on: module\nnodes: [{id: a, type: ai, provider: openai, model: m, prompt: x, api_key_env: GH_TOKEN}]",
                "collector's own secrets",
            ),
            ("on: module\nnodes: [{id: a, type: nope}]", "unknown type"),
            (
                "on: module\nnodes: [{id: a, type: osv, when: x}]",
                "isn't a field",
            ),
            (
                "on: module\nnodes: [{id: a, type: osv, next: b}]",
                "unknown node",
            ),
            (
                "on: module\nnodes: [{id: a, type: if, then: a, when: is_new}]",
                "cycle",
            ),
            (
                "on: module\nnodes: [{id: a, type: if, when: '(('}]",
                "needs `when` and `then`",
            ),
            (
                "on: module\nnodes: [{id: a, type: if, when: '((', then: b}, {id: b, type: osv}]",
                "bad condition",
            ),
            (
                "on: module\nnodes: [{id: a, type: log, message: '{{'}]",
                "bad message",
            ),
            (
                "on: module\nnodes: [{id: a, type: log, message: x, severity: loud}]",
                "severity",
            ),
            (
                "on: module\nnodes: [{id: a, type: osv}, {id: a, type: osv}]",
                "repeated",
            ),
        ];
        for (yaml, expected) in bad {
            let err = validate("w", &parse(yaml)).unwrap_err();
            assert!(err.contains(expected), "{yaml}: {err}");
        }
    }
}
