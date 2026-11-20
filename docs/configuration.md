# Configuration

| Variable | Used by | Purpose |
| --- | --- | --- |
| `OGHCOLLECTOR_TOKEN_GH` | collector | GitHub API token (fallback if the `gh_token` Docker secret isn't set) |
| `OGHCOLLECTOR_TOKEN_GL` | collector | GitLab API token (fallback if the `gl_token` Docker secret isn't set) |
| `DATABASE_URL` | Diesel CLI | SQLite connection string (local, non-Docker development only) |
| `OGHCOLLECTOR_DB_PATH` | server, mcp, migrate | Path to the SQLite database (default `data/data.db`); the MCP's first CLI argument takes precedence. The collector always writes to `data/data.db`. |
| `OGHCOLLECTOR_SCAN_ENABLED` | server, mcp | Enable live instance scans. Disabled by default. |
| `OGHCOLLECTOR_PROJECTS_ENABLED` | mcp | Enable the Doodba project tools (`list_doodba_projects`, `get_doodba_project`). Disabled by default (the server's switch is `projects_enabled`). |
| `OGHCOLLECTOR_MCP_BIND_ADDR` | mcp | HTTP bind address (default `0.0.0.0:8081`) |
| `OGHCOLLECTOR_MCP_ALLOWED_HOSTS` | mcp | Comma-separated `Host` header allowlist (default `localhost,127.0.0.1,::1`) |
| `OGHCOLLECTOR_MCP_KEY_SECRET` | server, mcp | HMAC secret of the per-user MCP API keys (or Docker secret `mcp_key_secret`). Set: `/mcp` needs `Authorization: Bearer <key>`, and the server's `/mcp` page shows each logged-in user theirs (needs `auth_enabled` and the numeric GitLab id as user header: oauth2-proxy `--user-id-claim=sub`). Unset: `/mcp` is open. Rotating it revokes every key |
| `OGHCOLLECTOR_MCP_GITLAB_URL` / `_GROUP` / `_TOKEN` / `_CACHE` | mcp | Required with the key secret: GitLab base URL, group path, read-only token (or Docker secret `mcp_gitlab_token`) and how many seconds a membership answer is cached (default 300). A user removed from the group, or blocked, loses access within that time |
| `OGHCOLLECTOR_MCP_CACHE_TTL` | mcp | Overrides `cache_ttl` from `mcp.yaml` (default `3600`) |
| `OGHCOLLECTOR_EMBED_CACHE_DIR` | collector, server, mcp | Directory for the embedding model cache (default `data/.fastembed_cache`) |
| `OGHCOLLECTOR_FORCE_REANALYZE` | collector | Reanalyze module sources even when their latest commit has not changed |
| `OGHCOLLECTOR_SCHEDULE_HOURS` | collector | `--listen`: collect every source in `collector.yaml` every this many hours, at multiples of it since 00:00 UTC (default `6`, `0` = off) |
| `OGHCOLLECTOR_WEBHOOK_SECRET_GH` / `_GL` | collector | `--listen`: GitHub/GitLab webhook secret (fallback if the `gh_webhook_secret`/`gl_webhook_secret` Docker secret isn't set); no secret = no endpoint |
| `OGHCOLLECTOR_WEBHOOK_BIND_ADDR` | collector | `--listen`: webhook bind address (default `0.0.0.0:8082`) |
| `OGHCOLLECTOR_CONFIG` | collector | Collector config a single-source run loads custom workflows from (default `collector`, i.e. `./collector.yaml`; set for you on the jobs of a `collector.yaml` run or of the daemon) |
| `OGHCOLLECTOR_WRITE_TOKEN_GH` / `_GL` | collector | Write token for workflow PR/MR actions (fallback if the `gh_write_token`/`gl_write_token` Docker secret isn't set); never the read token |
| `OGHCOLLECTOR_AI_LIMIT` | collector | AI calls per job at most (default `20`) |
| `VAULT_ADDR` / `VAULT_TOKEN` / `VAULT_NAMESPACE` / `VAULT_CACERT` | collector | HashiCorp Vault for `vault:` [workflow secrets](workflows.md#workflow-secrets): address, token (fallback if the `vault_token` Docker secret isn't set; none = a Vault Proxy adds it), Enterprise namespace, private CA (PEM) |
| `VAULT_ROLE_ID` / `VAULT_SECRET_ID` / `VAULT_APPROLE_PATH` | collector | AppRole login for `vault:` secrets: Docker secrets `vault_role_id`/`vault_secret_id` first, mount default `approle`. One login per job, short-lived token; wins over `VAULT_TOKEN` |
| `OGHCOLLECTOR_COMMIT_AUTHOR` | collector | Author of `pr_open` commits, `Name <email>` (default `OGHCollector <oghcollector@localhost>`) |
| `OGHCOLLECTOR_PR_ANALYSIS_LIMIT` | collector | Pull/merge requests analyzed per job at most, most recently active first (default `100`) |
| `OGHCOLLECTOR_SKIP_EMBEDDINGS` | collector | Skip the end-of-run embedding pass (set on the per-job children of a `collector.yaml` run, which runs it once at the end) |
| `RUST_LOG` | all three binaries | Log level (default: `info`) |

---

## Advanced Configuration

`docker-compose.yaml` is meant to stay untouched; layer your own settings (volumes, env vars, ports) on
top of it with a `docker-compose.override.yaml` file, which Docker Compose picks up automatically.
