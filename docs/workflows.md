# Workflows and actions

The collector itself only walks the sources: it clones them and finds every module folder. What
happens to each module is a *workflow*, a graph of nodes, and a source runs the ones it names. The
`default` workflow is what the collector always did (a test checks it leaves exactly the same data
and events), so a source without `workflows:` behaves as before; `workflows: []` only walks.

Workflows live in `workflows.yaml` at the repository root, next to `collector.yaml` (copies of the
`.example.yaml` files; in Docker both are mounted from there, so editing them needs no rebuild). The
binary carries no copy: `workflows.yaml` is the whole list (a workflow taken out of it is gone). `collector.yaml` may have a
`workflows:` too; its definitions replace any of the same name. A workflow with `enabled: false`
doesn't run even where a source names it (the job logs a warning); it's still checked. The shipped
`workflows.example.yaml` has `default` and one more, off: `ai_pr_review`, an AI reviewing the PRs that
bring grave security findings or migration notes (see [Actions](#actions)).

A workflow can run after others: `after: [default]`. On each module (or PR) it then runs once
those have, whatever the source's list order, only if they all *finished* (reached an end with no
node failing; stopping at an `if` without `else` is an end), and it starts from what they
computed: the manifest, analysis, findings, notes and AI answers are there without being computed
again (where two computed the same, the first in `after` wins). What it needs and they didn't
compute (say `default` on an unchanged module, which only stores the manifest) its own nodes
compute as usual, so a `security` node before a `security.grave` test costs nothing when
`default` already ran it. The workflows it names must be in the same source's list (they aren't
added for it), with the same trigger: PR workflows run after every module workflow anyway. One
that is off (or runs after one that is off) is skipped with a warning.

```yaml
# default (in workflows.yaml): unchanged modules only get their manifest stored
default:
  on: module
  nodes:
    - {id: changed, type: if, when: not unchanged, then: stats, else: store}
    - {id: stats, type: git_stats, next: analyze}
    - {id: analyze, type: analyze_code, next: security}
    - {id: security, type: security, next: migration}
    - {id: migration, type: migration_notes, next: store}
    - {id: store, type: store_module, next: osv}
    - {id: osv, type: osv}
```

A workflow starts at its first node and follows `next` (or, for `if`, `then`/`else`) until a node
has none. The portal can show them as a graph and build them visually: with
`workflow_editor_enabled` (server config), `/workflows` lists those of `workflows.yaml` and the collector config, lets you change them or draw new ones (drag nodes, link
their outputs, fill in their fields, switch them on or off) and gives the YAML to paste into
`workflows.yaml`, checked as you go by the same
rules as `oghcollector --check`. Nothing is saved there. Each node's place in the editor goes in
the YAML as `pos: [x, y]` (the collector ignores it), so a shared workflow opens laid out as it was
left; one without positions is laid out for you. Sources pick workflows by name (here
`workflows.yaml` and `collector.yaml` in one listing):

```yaml
workflows:
  security_only:                    # names: lowercase letters, digits, '-' and '_'
    on: module                      # each module a job finds (the only trigger for now)
    nodes:
      - {id: changed, type: if, when: not unchanged, then: stats, else: store}
      - {id: stats, type: git_stats, next: security}   # committers and folder size
      - {id: security, type: security, next: grave}
      - {id: grave, type: if, when: "security.grave > 0", then: alert, else: store}
      - id: alert
        type: log
        event: security
        severity: error
        message: "{{ technical_name }} [{{ odoo_version }}]: {{ security.grave }} grave finding(s)"
        next: store
      - {id: store, type: store_module}

sources:
  - source: MyGroup
    git: GL
    versions: [18.0]
    workflows: [security_only]
```

| Node | Does | Fields |
| --- | --- | --- |
| `if` | Goes to `then` when the `when` condition holds, else to `else` (none: ends) | `when`, `then`, `else` |
| `read_manifest` | Reads the manifest | `next` |
| `git_stats` | Committers and folder size, from git (without it they aren't updated: the module page's activity and size stay as stored) | `next` |
| `analyze_code` | Analyzes the `.py`/`.xml`/`.csv` files (views, models, records, routes) | `next` |
| `security` | [Security rules](reports.md) over that analysis | `next` |
| `migration_notes` | Migration notes for the collected version | `next` |
| `store_module` | Stores the module with what the workflow computed (a section it didn't compute keeps its stored data), its dependencies, and logs the usual events | `next` |
| `osv` | Known vulnerabilities of the pinned Python dependencies `store_module` just added | `next` |
| `log` | An event in `/logs`: `event` type (default `workflow`), `severity` (`info`, `success`, `warning`, `error`) and a `message` template | `next`, `event`, `severity`, `message` |
| `ai` | Asks an AI; the answer is `ai.<node id>` for the nodes after it. Outward: see [Actions](#actions) | `next`, `provider`, `model`, `prompt`, `system`, `url`, `api_key_env`, `max_tokens`, `timeout` |
| `pr_open` | Opens a PR/MR with one commit, the `patch` template (a unified diff, e.g. `{{ ai.fix }}`), on the module's version branch. Outward | `next`, `title`, `body`, `patch` |
| `apprise` | Sends a notification (`title`, `body` templates; `severity` as in `log`) through an [Apprise API](https://github.com/caronc/apprise-api) server at `url`. Outward: see [Actions](#actions) | `next`, `url`, `api_key_env`, `severity`, `title`, `body` |

`on: pull_request` workflows run on every PR/MR a job tracks (see the source's `pull_requests`),
after it's stored and analyzed. Besides `if`, `log`, `ai` and `apprise`, they have:

| Node | Does | Fields |
| --- | --- | --- |
| `pr_comment` | Comments on the PR/MR (`body` template). Outward | `next`, `body` |
| `pr_edit` | Replaces its description (`body` template; `pr.description` is the current one). Outward | `next`, `body` |
| `pr_review` | Reviews it: the `patch` template's changes (a unified diff against the PR's head, e.g. `{{ ai.review }}`) as suggestions its author applies in one click, `body` as the review's text. Comments only, never approves or asks for changes. Needs `pull_requests: full`. Outward | `next`, `body`, `patch` |

A node computes what it needs that no earlier node computed (`security` analyzes the code,
`store_module` reads the manifest). Conditions and messages are
[Jinja](https://docs.rs/minijinja/latest/minijinja/syntax/) over these variables:
`technical_name`, `path` (the module folder), `source_path`, `org`, `repo`, `odoo_version`,
`is_new` (not stored before), `unchanged` (same last commit as stored, analyzed by the same
analyses as the source's storing workflow),
`last_commit.hash|author|date|name`, `manifest.name|version|license|category|author|installable|depends`,
`security.grave|minor|findings` and `migration.count|notes`; the last three are empty until a node
computes them. `file('models/x.py')` returns a file of the module (up to 100 KB; nothing outside
its folder).

In `pull_request` workflows: `org`, `repo`, `odoo_version`,
`pr.number|title|author|labels|draft|ci_status|head_sha|head_ref|url|description`, `pr.is_new`
(first seen), `pr.head_changed`, `pr.ci_changed` (checks just turned green or red), `pr.is_ours`
(opened by `pr_open`), `pr.analyzed` (the analysis below is of this head: false while a new push
waits for it, and always on `pull_requests: migration` sources), and `modules`, one `{name, port, analysis}` per module it changes
(`analysis` with `pull_requests: full`). Over those, like a module workflow's own:
`security.grave|minor|findings` (only the findings the PR brings, not those of the merged module)
and `migration.count|notes`, each item with its `module` (empty without the analysis). `diff()` returns its changes (up to 200,000 characters;
`full` sources only, the others never fetch the PR head). Both kinds see `ai.<node id>`.

- `oghcollector --check` validates every workflow (types, fields, node references, cycles,
  conditions, templates) along with the sources. A source can store modules from one of its
  workflows only, and each workflow starts from a clean slate (they don't see each other's
  results).
- A module's stored analysis is tagged with which analyses produced it, so adding `security` or
  `migration_notes` to a source's workflow analyzes its unchanged modules once more.
- A failing node logs a warning and stops that workflow for that module; the job goes on.
- Stored modules a job no longer finds in the repository are deleted, whatever the workflows do.
- Which PRs/MRs are tracked (and analyzed) still depends on the source's `pull_requests`.

### Project workflows (`on: project`)

Routine fixes to a Doodba project, one commit per step and one PR/MR at the end. They run over
every Doodba project a job stores (see `doodba_fixes` in `workflows.yaml`, disabled; name it in a
source's `workflows:`). Besides `if`, `log`, `ai` and `apprise`, they have:

| Node | What it does | Fields |
|---|---|---|
| `drop_missing_addons` | Removes from `addons.yaml` the addons the collected repos no longer have at this version (`project.missing_addons`), in a commit | `next`, `title` (commit message) |
| `update_linters` | Moves `.pre-commit-config.yaml` hooks to the newest tag the fleet runs for this version (`project.lint_updates`; only tag → newer tag), in a commit | `next`, `title` |
| `pre_commit` | Runs the project's `pre-commit run --all-files` over the tree the steps above left and commits what its hooks fixed; the second pass must be clean, else the item's workflow stops (nothing is opened). Does nothing without earlier commits or a `.pre-commit-config.yaml`. Local, but hooks are foreign code: it only runs when `project_pr` is live | `next`, `title` |
| `project_pr` | Pushes the commits as `ogh/<workflow>/<version>-<tree>` and opens one PR/MR; nothing if no step changed anything. With `auto_merge: true` it also sets the PR/MR to merge when its pipeline/checks pass (never at once). Outward | `next`, `title`, `body`, `auto_merge` |

The fix nodes only commit in a scratch worktree, so even the dry-run report (`/logs`) shows the
real diffstat. `body` sees `changes` (`step`, `message`, `files` of each commit). Variables:
`org`, `repo`, `odoo_version`, `project.{answers,base_image,template_behind,addons,repos,linters,exposed,missing_addons,lint_updates}`,
`changes`, `ai.<node id>`, `file('rel/path')`. `after` isn't allowed on them.

- "Missing" means: listed in `addons.yaml`, its repo is collected at this version and lacks the
  module (a `private` addon: no folder under `odoo/custom/src/private`). It is **not** the project
  page's "Missing" column (absent from the *next* version): removing those would break the running
  deployment. A repo we don't collect is never "missing". Only list items are removed
  (flow-style lists are left alone); `*`, `ENV` and `ONLY` are never touched.
- The files are edited as text, so comments and formatting survive.
- A PR/MR of the same workflow and version that is still open blocks a new one; the branch name
  carries the resulting tree, so the same fixes find their earlier push. GitLab MRs remove their
  source branch when merged.
- `pre_commit`: needs the `pre-commit` binary and network (hook repos) in the collector image; scrubbed environment (no forge tokens), 10 min per pass, hook cache in `OGHCOLLECTOR_PRE_COMMIT_HOME` (default: the job's temp dir, so cold each job). A hook that fixes files exits 1: pass 1 failing with changes is normal, with none it is a real failure.
- `auto_merge`: GitLab needs a pipeline on the MR (it waits ~30 s for one, then fails and
  retries next run: without one GitLab would merge at once) and uses "merge when pipeline
  succeeds"; GitHub uses GraphQL `enablePullRequestAutoMerge` (merge commit), which needs
  auto-merge enabled on the repository and required checks, and is refused otherwise. There is no
  fallback to a direct merge. A failure (no pipeline yet) is retried next run on the same open PR/MR.
- Not covered: updating the copier template (it runs the template's tasks, i.e. foreign code) and
  dropping repos left empty in `repos.yaml`.

### Actions

`ai`, `pr_comment`, `pr_edit`, `pr_review`, `pr_open`, `apprise` and `project_pr` reach outside the collector, so they only **log what
they would do** (`[dry-run] ...` events in `/logs`, with what they'd send) until the source lists
them:

```yaml
workflows:
  review:
    on: pull_request
    nodes:
      - {id: ours, type: if, when: "not pr.is_ours", then: ask}
      - id: ask
        type: ai
        provider: anthropic              # or openai: OpenAI, Ollama, LM Studio, vLLM...
        model: claude-opus-5-5
        api_key_env: ANTHROPIC_API_KEY   # env var, or the anthropic_api_key Docker secret
        prompt: "Review this Odoo {{ odoo_version }} change:\n{{ diff() }}"
        next: comment
      - {id: comment, type: pr_comment, body: "{{ ai.ask }}"}
  suggest:
    on: pull_request
    nodes:
      - {id: ours, type: if, when: "not pr.is_ours", then: ask}
      - id: ask
        type: ai
        provider: anthropic
        model: claude-opus-5-5
        api_key_env: ANTHROPIC_API_KEY
        prompt: |
          Odoo {{ odoo_version }} PR. Reply with a unified diff against its head that fixes
          migration problems, changing only lines this diff shows, or nothing if all is fine:
          {{ diff() }}
          Migration notes: {{ modules | map(attribute='analysis') | list | tojson }}
        next: review
      - {id: review, type: pr_review, body: "AI suggestions: check them before applying.", patch: "{{ ai.ask }}"}
  fix_views:
    on: module
    nodes:
      - {id: todo, type: if, when: "not unchanged", then: notes}
      - {id: notes, type: migration_notes, next: any}
      - {id: any, type: if, when: "migration.count > 0", then: ask}
      - id: ask
        type: ai
        provider: openai
        url: http://ollama:11434/v1      # a local model
        model: qwen2.5-coder
        prompt: "Write a unified diff (paths from the repo root, under {{ source_path }}/) fixing: {{ migration.notes | tojson }}"
        next: open
      - {id: open, type: pr_open, title: "[{{ odoo_version }}][FIX] {{ technical_name }}: migration notes", patch: "{{ ai.ask }}"}

sources:
  - source: MyGroup
    git: GL
    versions: [18.0]
    pull_requests: full
    workflows: [default, fix_views, review, suggest]
    actions: [ai, pr_comment, pr_review] # pr_open stays dry-run
```

- **Once per item.** A ledger (`workflow_action`) records every action: a comment or description
  edit runs once per PR head (a new push may get another), `pr_open` once per module and version,
  and dry-run reports once too. An AI answer is kept and reused while the item, prompt and settings
  are the same, so an unchanged module or PR never pays twice. A failed action isn't recorded and
  is retried on the next run.
- **Tokens.** PR/MR actions use a write token of their own, never the read one:
  `OGHCOLLECTOR_WRITE_TOKEN_GH` / `_GL` (or the `gh_write_token` / `gl_write_token` Docker secret);
  without it they fail with a warning. AI keys come from the secret named in `api_key_env` (see
  below); none of them is ever logged.
- <a id="workflow-secrets"></a>**Secrets.** A workflow names its secrets, never holds them:
  `api_key_env`, and `${...}` anywhere in a `url` after its host, take either a name (the Docker
  secret of that name in lowercase, else the env var) or `vault:<path>#<key>`, read from
  [HashiCorp Vault](https://developer.hashicorp.com/vault) (or OpenBao) at its API path
  `/v1/<path>`, so a KV v2 secret has `data/` after the mount (`vault kv put secret/ogh
  anthropic_api_key=...` → `vault:secret/data/ogh#anthropic_api_key`; KV v1 has no `data/`).
  They're read only by the collector, only right before a live call (never in dry-run, never for
  an answer the ledger already has), once per job; never in templates, logs, `/logs`, the ledger
  or `/workflows`. The collector's own secrets (`OGHCOLLECTOR_*`, `VAULT_*`, the forge tokens) are
  refused. Vault is set with its usual variables on the collector: `VAULT_ADDR`, `VAULT_TOKEN` (or
  the `vault_token` Docker secret), `VAULT_NAMESPACE` (Enterprise) and `VAULT_CACERT` (a private
  CA's PEM). Give the token a policy that only reads those paths; for AppRole, Kubernetes and
  token renewal, run a [Vault Proxy](https://developer.hashicorp.com/vault/docs/agent-and-proxy/proxy)
  with `api_proxy { use_auto_auth_token = true }` and point `VAULT_ADDR` at it, with no token.

  ```yaml
  - id: ask
    type: ai
    provider: anthropic
    model: claude-opus-5-5
    api_key_env: vault:secret/data/ogh#anthropic_api_key
    prompt: "..."
  - id: tell
    type: apprise
    url: http://apprise:8000/notify/${vault:secret/data/ogh#apprise_key}
    body: "..."
  - {id: tell2, type: apprise, url: 'http://apprise:8000/notify/${APPRISE_KEY}', body: "..."}  # quoted inside { }
  ```
- **Limits.** A job makes at most `OGHCOLLECTOR_AI_LIMIT` (default `20`) AI calls, each waiting up
  to `timeout` seconds (default `300`, at most `600`); `max_tokens` defaults to `16000`. Items past
  the limit wait for the next run.
- **Gates that are true once.** `pr.head_changed`, `pr.ci_changed` and `not unchanged` only hold on
  the run that sees the change: an action that fails then (an AI timeout, the call limit, a missing
  token) isn't retried for that head or commit, and turning a node live doesn't act on items already
  seen. The ledger already stops repeats, so gate on them only when that's what you mean. A module
  workflow gated on `not unchanged` (like `fix_views`) only ever looks at modules with new commits;
  without the gate it analyzes every module on every run (its AI answers and PRs are still once).
- **Bot PRs.** A PR opened or edited by these nodes comes back through the webhooks; gate PR
  workflows on `not pr.is_ours` so they don't review their own work.
- **The demo, `ai_pr_review`** (in `workflows.yaml`): on PRs that aren't its own and bring a
  grave security finding or a migration note, it asks `claude-opus-5-5` for a fix as a diff of the
  PR's lines and posts a review listing the findings, with the fix as suggestions. It ships with
  `enabled: false`: to try it, set `enabled: true` and name it in a `pull_requests: full` source
  (`workflows: [default, ai_pr_review]`), which only logs what it would ask and post; then add
  `actions: [ai]` to have it ask (`/logs` then says how many suggestions it would post, on which
  files), and `pr_review` to post them. Set
  `ANTHROPIC_API_KEY` (or the `anthropic_api_key` Docker secret) and the write token. Change it
  in `workflows.yaml` itself, or build a copy at `/workflows`.
- **`pr_review` suggestions.** Each hunk of the diff is found by its text in the file at the PR's
  head (an AI's line numbers are often off), trimmed to the lines it changes, and posted as a
  one-click suggestion (GitHub: one review, `event: COMMENT`; GitLab: a thread each, then a note).
  The forges only take them on lines the PR's own diff shows: whatever falls outside, isn't found,
  or adds or deletes whole files goes in the review's text as a diff to apply by hand, and so does
  a review GitHub refuses (one bad line sinks it: it's sent again as text) or a GitLab thread it
  refuses. A GitLab MR pushed to since the PR step waits for the next run (its new head). Dry-run
  logs how many suggestions it would make and on which files; it only has something to say when
  `ai` is live (a dry-run `ai` answers "").
- **`apprise` notifications** go through an Apprise API server (e.g. the `caronc/apprise` image
  on the compose network, not published): `url: http://apprise:8000/notify/ogh` sends to what its
  configuration `ogh` lists, so no service's token is in the workflow (its key is: write it as
  `/notify/${APPRISE_KEY}`, see secrets above); or `url:
  http://apprise:8000/notify/` with `api_key_env` naming the secret that holds
  the Apprise URLs (`tgram://...`, `mailto://...`, comma-separated). `severity: error` goes out as
  Apprise's `failure`. Once per item like the rest (one per module commit or PR head and node),
  so put it behind an `if` (`is_new`, `security.grave > 0`, `pr.is_new`...). A job sends at most
  20: past that the node is skipped and the workflow goes on, so the rest only go out on a later run
  if their gate still holds (see the gates that are true once, below). An answer of 424 (some of
  its targets failed) counts as sent; one that sent nothing (Apprise down) stops that item's
  workflow and is retried, so place it after `store_module`/`osv`, or in a workflow of its own with
  `after: [default]`, where an outage can't hold up collecting.
- **`pr_edit` replaces** the description. A `body` that appends to `pr.description` grows on every
  new head unless it checks for its own text: `{% if 'Reviewed' not in pr.description %}...`.
- **Dry-run reports** are capped at 20 per job; the rest are reported by later runs (a line says how
  many were held back).
- **`pr_open` safety.** The branch is always `ogh/<workflow>/<module>-<version>`, never pushed
  over an existing one (an existing branch is an earlier attempt: only its missing PR is created).
  The patch must only add, edit or delete regular files inside the module folder (no renames out of
  it, no mode changes, symlinks, submodules or binaries) and apply cleanly; dry-run checks all that
  and logs which files it would change. It's committed as `OGHCOLLECTOR_COMMIT_AUTHOR` ("Name
  <email>", default `OGHCollector <oghcollector@localhost>`) in a worktree of its own. **A branch
  pushed to the repository runs its CI with the repository's secrets**, and AI-written code (whose
  prompt may include text from anyone's PR) then runs there: enable `pr_open` only where that's
  acceptable.
- Sending a private repository's code to a cloud AI is up to you: it only happens for sources
  that list `ai`, to the `url`/provider the workflow names.
