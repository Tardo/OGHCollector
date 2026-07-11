<h1 align="center">Odoo GitHub Collector</h1>

<p align="center">
  Collects Odoo module metadata from GitHub/GitLab organizations, visualizes it on a web dashboard,
  and serves it to LLM clients over MCP.
</p>

<p align="center">
  <a href="https://github.com/Tardo/OGHCollector/actions/workflows/tests.yml"><img alt="Tests" src="https://github.com/Tardo/OGHCollector/actions/workflows/tests.yml/badge.svg"></a>
  <a href="./COPYING"><img alt="License: AGPL v3" src="https://img.shields.io/badge/license-AGPLv3-blue.svg"></a>
</p>

---

## Overview

The project is a Rust workspace made of three services that share a single SQLite database:

| Service | Binary | Role |
| --- | --- | --- |
| **OGHCollector** | `oghcollector` | CLI that clones repositories, parses Odoo `__manifest__.py` files and writes the results to the database. It is the **only** component allowed to write. |
| **OGHServer** | `oghserver` | actix-web dashboard that reads the database in read-only mode (module search, dependency graph, migration tracking, committer stats, etc). |
| **OGHMcp** | `oghmcp` | Read-only [MCP](https://modelcontextprotocol.io/) server exposing module search and code-analysis data as tools, over a Streamable HTTP endpoint (`/mcp`), so any LLM client can reach it by URL. |

All three ship in the same Docker image and the project is designed to be run with Docker Compose.

---

## Requirements

1. [Docker](https://docs.docker.com/get-docker/) and the Docker Compose plugin.
2. A GitHub and/or GitLab personal access token so the collector can query the API:
   - [Creating a GitHub personal access token](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)
   - A personal access token for your GitLab instance, if you also collect from GitLab.

---

## Quick Start

```sh
git clone git@github.com:Tardo/OGHCollector.git
cd OGHCollector

# 1. Build the images
docker compose build

# 2. Populate the database at least once — this also applies any pending Diesel
#    migrations automatically, see "Database & Migrations" below
docker compose run --rm -u appuser -T app oghcollector OCA 18.0

# 3. Start the dashboard (:8080) and the MCP endpoint (:8081)
docker compose up
```

The dashboard is now available at `http://localhost:8080` and the MCP endpoint at `http://localhost:8081/mcp`.

> `oghserver` and `oghmcp` open the database **read-only**. If it doesn't exist yet, they will
> start anyway but won't serve any data until `oghcollector` has run at least once.

---

## Database & Migrations (Diesel)

The schema is managed with [Diesel](https://diesel.rs/); migration files live in [`migrations/`](./migrations).

### With Docker (the normal workflow)

Migrations are compiled straight into the `oghcollector` binary via `diesel_migrations::embed_migrations!`
and are applied automatically, in order, every time the collector runs. **There is nothing to run by hand:**
running the collector once creates the database file, and every later run — including after you `git pull`
a version of this project that adds new migrations — brings the schema up to date before it does anything else.

```sh
docker compose run --rm -u appuser -T app oghcollector OCA 18.0
```

### Local (non-Docker) development

If you're running the crates directly with `cargo run` instead of Docker, install the Diesel CLI and use
the `DATABASE_URL` already defined in [`.env`](./.env):

```sh
cargo install diesel_cli --no-default-features --features sqlite-bundled

diesel migration run       # apply pending migrations
diesel migration revert    # undo the last migration
diesel migration list      # show applied/pending migrations
```

> `crates/sqlitedb/src/schema.rs` is **manually maintained**, not regenerated from `diesel print-schema`.
> Diesel infers SQLite `INTEGER PRIMARY KEY` columns as `Nullable<Integer>`, but every id in this project
> is a non-null `i64`/`BigInt`. If you run `diesel print-schema`, diff it by hand and port over only the
> new `table!` block(s) — never overwrite `schema.rs` with the raw output.

See [`docs/development.md`](./docs/development.md) for the full non-Docker development setup.

---

## OGHServer

### Configuration

Mount a volume to `/app/server.yaml` (JSON is also supported) to override the defaults:

| Name | Type | Description | Default |
| --- | --- | --- | --- |
| `bind_address` | string | Address to bind the server on | `0.0.0.0` |
| `port` | int | Port to bind the server on | `8080` |
| `workers` | int | Number of worker processes | `2` |
| `template_autoreload` | bool | Reload templates automatically when they change | `false` |
| `static_autoreload` | bool | Reload static files automatically when they change | `false` |
| `allowed_origins` | list of strings | Allowed CORS origins | `[]` |
| `timezone` | string | Timezone used for display | `UTC` |
| `cookie_key` | string | Key used to sign session cookies | |
| `upload_limit` | int | Maximum upload size, in bytes | `2097152` |
| `cache_ttl` | int | Seconds a cache entry stays valid | `3600` |
| `db_pool_max_size` | int | Maximum number of pooled DB connections | `15` |
| `mcp_info_enabled` | bool | Show the `/mcp` page explaining how to connect popular LLM clients to the MCP endpoint, and its nav link | `false` |
| `mcp_url` | string | Public URL of the MCP endpoint, displayed on that page | `http://localhost:8081/mcp` |
| `trusted_proxies` | list of strings | IPs/CIDRs (e.g. your reverse proxy's address, or the Docker network subnet) allowed to set `X-Forwarded-For`/`Forwarded`; honored only when the request's direct TCP peer matches one of these, otherwise the headers are stripped and the real peer address is used instead. Needed for correct client IPs in access logs (and `REQ_BASE_URL` scheme/host) behind Traefik/nginx/etc. | `[]` |
| `seo_enabled` | bool | Allow search engines/social previews to index and share the site: `/robots.txt` returns `Allow: /` instead of `Disallow: /`, and pages get a canonical link plus Open Graph/Twitter Card meta tags. Off by default so nothing is shared/indexed until explicitly opted in. | `false` |

```yaml
# docker-compose.override.yaml
services:
  app:
    volumes:
      - ./server.yaml:/app/server.yaml
```

---

## OGHCollector

### Usage

```sh
docker compose run --rm -u appuser -T app oghcollector <origin> <version> [git_type]
```

- `<origin>`:
  - The name of an organization — all its repositories are scanned.
  - The name of a repository, optionally followed by `:` and a comma-separated list of folders to scan. Each folder must start with `/` (it's appended directly to the clone path). To scan the repo root *as well as* subfolders, add a trailing comma (an empty entry means the root): `:/addons,` scans `/addons` plus the root.
- `<version>`: Odoo version to collect (e.g. `18.0`).
- `[git_type]`: Optional git client to use, `GH` (GitHub, default) or `GL:<api_url>` (GitLab).

### Examples

```sh
# Odoo core modules, 18.0 (GitHub)
docker compose run --rm -u appuser -T app oghcollector odoo/odoo:/addons,/odoo/addons 18.0

# OCA/web modules, 18.0 (GitHub)
docker compose run --rm -u appuser -T app oghcollector OCA/web 18.0

# All OCA modules, 18.0 (GitHub)
docker compose run --rm -u appuser -T app oghcollector OCA 18.0

# All MyGroup modules, 18.0 (self-hosted GitLab)
docker compose run --rm -u appuser -T app oghcollector MyGroup 18.0 GL:https://mygitlabinstance.com/api/v4/
```

> If you run this behind Traefik, you may need to add `-l traefik.enable=false` so the one-off
> container isn't picked up as a routable service.

### Authentication

The recommended way to provide API tokens is through Docker secrets, so they never end up in
`docker-compose.yaml` or shell history. The collector automatically reads `/run/secrets/gh_token` and
`/run/secrets/gl_token` if present:

```yaml
# docker-compose.override.yaml
services:
  app:
    secrets:
      - gh_token
      - gl_token

secrets:
  gh_token:
    file: ./gh_token.txt
  gl_token:
    file: ./gl_token.txt
```

Make sure each secret file contains a single line with no extra trailing newline added by your editor
(e.g. `nano -L gh_token.txt`).

Alternatively, without secrets, set `OGHCOLLECTOR_TOKEN_GH` / `OGHCOLLECTOR_TOKEN_GL` as environment
variables (used as a fallback when the corresponding secret file isn't found).

### Scheduling updates

To refresh the database periodically, add a cron job on the host that invokes [`update_db.sh`](./update_db.sh),
which loops over every supported Odoo/OpenERP version for `odoo/odoo` and `OCA`:

```cron
0 */6 * * * cd /path/to/OGHCollector && ./update_db.sh
```

---

## OGHMcp

Streamable HTTP MCP endpoint exposing `search_modules` and `get_module` tools over the same category of
data as `oghserver`'s `/api/v1/*` routes.

### Configuration

Mount a volume to `/app/mcp.yaml` (JSON is also supported) to override the defaults. Every key can also be
set via an `OGHCOLLECTOR_MCP_`-prefixed environment variable (e.g. `OGHCOLLECTOR_MCP_CACHE_TTL`), which
takes precedence over the file:

| Name | Type | Description | Default |
| --- | --- | --- | --- |
| `cache_ttl` | int | Seconds the `get_module` result cache stays valid | `3600` |

```yaml
# docker-compose.override.yaml
services:
  mcp:
    volumes:
      - ./mcp.yaml:/app/mcp.yaml
```

By default the MCP server only accepts requests whose `Host` header is `localhost`, `127.0.0.1` or `::1`
(DNS-rebinding protection). For any real deployment, set `OGHCOLLECTOR_MCP_ALLOWED_HOSTS` to the
hostname(s)/IP(s) your clients actually connect to:

```yaml
# docker-compose.override.yaml
services:
  mcp:
    environment:
      OGHCOLLECTOR_MCP_ALLOWED_HOSTS: mcp.example.com,203.0.113.10
```

> `oghmcp` has no authentication of its own. If `oghserver` sits behind auth or a private network,
> give `oghmcp` the same treatment.

---

## Environment Variables

| Variable | Used by | Purpose |
| --- | --- | --- |
| `OGHCOLLECTOR_TOKEN_GH` | collector | GitHub API token (fallback if the `gh_token` Docker secret isn't set) |
| `OGHCOLLECTOR_TOKEN_GL` | collector | GitLab API token (fallback if the `gl_token` Docker secret isn't set) |
| `DATABASE_URL` | Diesel CLI | SQLite connection string (local, non-Docker development only) |
| `OGHCOLLECTOR_DB_PATH` | mcp | Path to the SQLite database (default `data/data.db`) |
| `OGHCOLLECTOR_MCP_BIND_ADDR` | mcp | HTTP bind address (default `0.0.0.0:8081`) |
| `OGHCOLLECTOR_MCP_ALLOWED_HOSTS` | mcp | Comma-separated `Host` header allowlist (default `localhost,127.0.0.1,::1`) |
| `OGHCOLLECTOR_MCP_CACHE_TTL` | mcp | Overrides `cache_ttl` from `mcp.yaml` (default `3600`) |
| `RUST_LOG` | all three binaries | Log level (default: `info`) |

---

## Advanced Configuration

`docker-compose.yaml` is meant to stay untouched; layer your own settings (volumes, env vars, ports) on
top of it with a `docker-compose.override.yaml` file, which Docker Compose picks up automatically.

---

## Development

See [`docs/development.md`](./docs/development.md) for running the crates and the frontend build directly
with Cargo/pnpm, outside of Docker.

## License

Distributed under the terms of the [GNU AGPLv3](./COPYING).
