# OGHCollector

## Usage

```sh
docker compose exec -T collector oghcollector                      # every source in collector.yaml
docker compose exec -T collector oghcollector --config <file>      # every source in another config file
docker compose exec -T collector oghcollector --check              # validate the config, print its jobs
docker compose exec -T collector oghcollector <origin> <version> [git_type]  # a single source/version
```

Without a source, the collector runs every job declared in `collector.yaml` (start from [`collector.example.yaml`](../collector.example.yaml); see [Collector config](#collector-config)). The single-run form takes:

- `<origin>`:
  - The name of an organization — all its repositories are scanned (on GitLab, only the projects directly in the group: list each subgroup as its own source), up to 12,700 GitHub repositories or 25,400 GitLab projects.
  - The name of a repository (`owner/repo`; on GitLab also `group/subgroup/project`), optionally followed by `:` and a comma-separated list of folders to scan. On GitLab, `group/subgroup` that isn't a project is scanned as a group. Each folder must start with `/` (it's appended directly to the clone path). To scan the repo root *as well as* subfolders, add a trailing comma (an empty entry means the root): `:/addons,` scans `/addons` plus the root.
- `<version>`: Odoo version to collect (e.g. `18.0`).
- `[git_type]`: Optional git client to use: `GH` (GitHub, default), `GL` (gitlab.com), or `GL:<api_url>` (a GitLab API URL).

> GitLab collections use the first segment of a project's path as the organization, so two projects
> with the same name in different subgroups of one group are stored as the same repository: collect
> only one of them.

## Examples

```sh
# Odoo core modules, 18.0 (GitHub)
docker compose exec -T collector oghcollector odoo/odoo:/addons,/odoo/addons 18.0

# OCA/web modules, 18.0 (GitHub)
docker compose exec -T collector oghcollector OCA/web 18.0

# All OCA modules, 18.0 (GitHub)
docker compose exec -T collector oghcollector OCA 18.0

# All MyGroup modules, 18.0 (self-hosted GitLab)
docker compose exec -T collector oghcollector MyGroup 18.0 GL:https://mygitlabinstance.com/api/v4/
```

## Collector config

`collector.yaml` declares what to collect: organizations/groups or repositories, the Odoo versions
of each and the folders that hold their modules. It's what the scheduled job runs.

```yaml
templates:                          # named module-folder layouts, written once
  odoo: [addons, odoo/addons]
  addons: [addons]

sources:
  - source: odoo/odoo               # organization/group, or owner/repo
    template: odoo                  # module folders from `templates`
    versions: [17.0, 18.0, 19.0]    # Odoo versions (branches) to collect
  - source: OCA                     # no template/paths: modules at each repo's root
    versions: [18.0]
  - source: Acme/erp
    template: addons
    paths: [third_party]            # extra folders, added after the template's
    versions: [18.0]
  - source: MyGroup/odoo            # GitLab: a project, or a subgroup (its projects, not its subgroups')
    git: GL                         # GH (default) or GL
    base_url: https://mygitlabinstance.com/api/v4/  # self-hosted GitLab (default: gitlab.com)
    template: addons
    versions: [18.0]
    workspaces: [mygroup]           # portal workspaces it's listed in (default: all)
    pull_requests: full             # every PR/MR analyzed (default: migration, see below)
```

`workspaces` groups repositories in the portal, see [Workspaces](auth.md).

Folders are relative to each repository root (a leading `/` is optional, `""` or `.` is the root).
A repository whose layout changed between versions (OpenERP's `openerp/addons` vs Odoo's
`odoo/addons`) is listed once per layout, each with its own `versions`.

The schema is meant to be generated as well as hand-written: every value has a fixed type, the
reuse is by name (`template`) rather than YAML anchors, unknown keys are errors (a typo never falls
back silently to a default), and the same keys work as JSON or TOML (`--config collector.json`).
`oghcollector --check` validates the file and prints its jobs, one per line, without running them
(exit status `2` on an invalid file), so a pipeline can check a generated config before deploying it.

The file is validated before anything runs. Jobs then run in order, one per source,
`oghcollector <source>[:<paths>] <version>,<version>... <git>`. A job first lists the source and
fetches each repository once, only the branches of its versions that exist (a repository without
one of them simply has nothing for that version, not an error); then each version runs as its
own child process over those clones, checked out locally. A failed or crashed version is logged
and skipped, the other versions and jobs still run, and the run exits non-zero at the end. The
embedding pass runs once, after the last job.

A job fails when anything goes wrong in it, not only when it crashes: an API error (a bad or
expired token, a rate limit that outlasts the retries), a repository that can't be cloned or
fetched, a busy lock, a missing token, a crash, an out-of-memory kill or a hang (a job still
running after 4 hours is killed; stalled HTTP requests and git transfers time out on their own
well before that). What it did collect is
kept. Each failed job is also recorded as a `Scan failed for '<source>' [<version>]: <first error>`
error in the portal's event log (`/logs`, and its Atom feed). API rate limits are waited out
(up to an hour) and server/network errors retried before giving up.

`workflows.yaml` is always **required** and `collector.yaml` whenever the collector has to read the sources from it (no source argument, the `--listen` daemon): without them it fails with an error saying which file is missing. A single run with arguments (`oghcollector OCA 18.0`) needs no `collector.yaml`. The
image doesn't contain them; `docker-compose.yaml` bind-mounts `./collector.yaml` and `./workflows.yaml`
read-only into `/app` (collector and server), so edits apply on the next run without rebuilding. Create
them from `collector.example.yaml` and `workflows.example.yaml` before `docker compose up`; if one is
missing, Docker creates a directory with that name and the service fails. For plain `docker run`, mount
them the same way (`-v ./collector.yaml:/app/collector.yaml:ro`). If your editor saves by replacing the
file (vim, most IDEs), the container keeps seeing the old copy until `docker compose restart collector`.

## Scheduling updates

The `collector` service runs `oghcollector --listen`, which stays up and runs collection jobs one
at a time, each as its own child process (so a failed or hung job is recorded and skipped exactly
as in a [`collector.yaml`](#collector-config) run):

- **Schedule**: every source in `collector.yaml` at 00:00, 06:00, 12:00 and 18:00 UTC (every
  multiple of `OGHCOLLECTOR_SCHEDULE_HOURS`, default `6`; `0` turns it off). By default that's
  OpenERP (6.1–9.0), Odoo (10.0–20.0) and OCA (6.1–20.0). Nothing runs at startup, so a redeploy
  doesn't start a batch, except on an empty database (a new install). The log says when the next
  run starts. A redeploy kills the running job and drops the pending ones: the batch starts over at
  the next scheduled time. A job still waiting from the previous batch isn't queued twice.
- **Webhooks**: a push to a version branch (`18.0`, ...) of a repository some source collects, a
  pull/merge request against one, or its CI finishing, queues that repository and version alone,
  ahead of any scheduled job still waiting, so it starts as soon as the running job ends. Several
  deliveries for the same repository and version while it waits are one job. The embedding pass
  runs whenever the queue empties, unless `OGHCOLLECTOR_SKIP_EMBEDDINGS` is set on the daemon.

Each job also refreshes the open pull/merge requests of its repositories and version, as the
source's `pull_requests` says:

- `migration` (default, meant for public sources like OCA and Odoo): only requests on an OCA port
  branch (`18.0-mig-<module>`), with their CI. Nothing is fetched nor analyzed.
- `full` (meant for your own repositories): every request. It's mapped to the modules it changes
  (from its diff, no extra API calls) and each module is analyzed at the request's head with the
  same rules as merged code; its code is never checked out in the shared clone, only extracted into
  the job's own temporary directory, and its manifest is parsed, not executed.

Requests whose head didn't change aren't analyzed again, and a job analyzes at most `OGHCOLLECTOR_PR_ANALYSIS_LIMIT` (default `100`; repository
by repository, each one's most recently active first) and looks up at most 500 GitHub CI states (2
requests each, a share of the token's hourly limit): on the first run the backlog is spread over the
next ones, so the first runs after upgrading are slower and a request only shows its analysis once
its turn comes. The module
page lists them in its *Pull requests* tab. For `full` sources, CI turning green or red,
merges/closes and grave findings a request introduces are logged as `pull_request` events (`/logs`, the feed with
`type=pull_request`), only against a state seen before, so the first run over thousands of open
requests stays quiet.

`collector.yaml` is read again for every scheduled run and webhook, so editing it needs no restart.
Follow it with `docker compose logs -f collector`; failed jobs are also in the portal's event log.
A manual `docker compose exec -T collector oghcollector ...` still works, but it fails on the org
lock while the daemon is collecting the same org.

### Webhooks

You can only add webhooks to repositories or organizations/groups you administer, so OCA and Odoo
stay on the schedule. Expose port `8082` (behind your HTTPS reverse proxy), set a secret per
provider, and add a hook with the same secret and these events:

- GitHub: *Pushes*, *Pull requests*, *Check suites* and *Statuses*.
- GitLab: *Push events*, *Merge request events* and *Pipeline events*.


| | Hook URL | Secret | Where |
| --- | --- | --- | --- |
| GitHub | `https://<host>/hooks/github`, content type `application/json` | `gh_webhook_secret` Docker secret or `OGHCOLLECTOR_WEBHOOK_SECRET_GH` | Repository or organization → *Settings → Webhooks* |
| GitLab | `https://<host>/hooks/gitlab` | `gl_webhook_secret` Docker secret or `OGHCOLLECTOR_WEBHOOK_SECRET_GL` (*Secret token*) | Project or group → *Settings → Webhooks* (group hooks need Premium) |

```yaml
# docker-compose.override.yaml
services:
  collector:
    secrets:
      - gh_webhook_secret

secrets:
  gh_webhook_secret:
    file: ./gh_webhook_secret.txt   # e.g. `openssl rand -hex 32`
```

A provider without a secret has no endpoint (and with neither, nothing listens): a valid delivery
starts clones, so unsigned ones are never accepted. GitHub deliveries are checked against their
`X-Hub-Signature-256` HMAC, GitLab ones against `X-Gitlab-Token`. A CI event only names a commit,
so it's mapped through the stored head of an open request (unknown commits are ignored). Other
events, other branches, deleted branches, unfinished CI and repositories no source collects get a
`200` and are ignored; a queued one gets a `202` listing its jobs (visible under the hook's *Recent
deliveries*).

The queue lives in memory: deliveries still waiting when the collector restarts are dropped, and
the next scheduled run catches up.
