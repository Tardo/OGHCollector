# OGHServer

## Configuration

Mount a volume to `/app/server.yaml` (JSON is also supported) to override the defaults:

| Name | Type | Description | Default |
| --- | --- | --- | --- |
| `bind_address` | string | Address to bind the server on | `0.0.0.0` |
| `port` | int | Port to bind the server on | `8080` |
| `workers` | int | Number of worker processes | `2` |
| `template_autoreload` | bool | Reload templates automatically when they change | `false` |
| `allowed_origins` | list of strings | Allowed cross-origin CORS origins. Empty denies cross-origin browser access. | `[]` |
| `cookie_key` | string | Key used to sign session cookies; must be at least 64 bytes, otherwise a new key is generated at startup | |
| `upload_limit` | int | Maximum upload size, in bytes | `2097152` |
| `cache_ttl` | int | Seconds a cache entry stays valid | `3600` |
| `db_pool_max_size` | int | Maximum number of pooled DB connections | `15` |
| `doodba_max_modules` | int | Maximum number of modules accepted per Doodba tool request (converter/dependency-resolver/migration-plan); larger requests are rejected with `400` | `500` |
| `mcp_info_enabled` | bool | Show the `/mcp` page explaining how to connect popular LLM clients to the MCP endpoint, and its nav link | `false` |
| `workflow_editor_enabled` | bool | Show the `/workflows` editor (see [Workflows](workflows.md)) and its nav link. **It publishes the `workflows:` section of the collector config** (prompts, AI hosts, `api_key_env` names, Apprise `url`s: write their keys as `${...}` [secrets](workflows.md#workflow-secrets)), never its sources or actions, nor any secret's value | `false` |
| `collector_config` | string | Collector config the editor reads the workflows from (`config` crate path, extension optional) | `collector` |
| `scan_enabled` | bool | Enable the live instance scanner. Disabled by default because it creates outbound requests; enable it only behind the access controls appropriate for your deployment. | `false` |
| `projects_enabled` | bool | Enable the Doodba projects pages (`/projects`), their `/v1/projects` and `/v1/project/{org}/{repo}/{version}` API and the "Used by N Doodba projects" line on module pages: the template drift, aggregated repos, patches (with their PR/MR state upstream), addons, linters, dependencies, PyPI advisories of the pip pins and credentials found in the files of every collected Doodba project. Disabled by default: it describes your private deployments, enable it only behind access controls (MCP: `OGHCOLLECTOR_PROJECTS_ENABLED`). | `false` |
| `mcp_url` | string | Public URL of the MCP endpoint, displayed on that page | `http://localhost:8081/mcp` |
| `trusted_proxies` | list of strings | IPs/CIDRs (e.g. your reverse proxy's address, or the Docker network subnet) allowed to set `X-Forwarded-For`/`Forwarded`; honored only when the request's direct TCP peer matches one of these, otherwise the headers are stripped and the real peer address is used instead. Needed for correct client IPs in access logs (and `REQ_BASE_URL` scheme/host) behind Traefik/nginx/etc. | `[]` |
| `auth_enabled` | bool | Require a login: every request needs the user header set by the auth proxy (else `401`) and workspaces follow the user's groups. See [Login with OIDC](auth.md#login-with-oidc). Startup fails without `trusted_proxies`. | `false` |
| `auth_user_header` | string | Header carrying the user id | `X-Auth-Request-User` |
| `auth_groups_header` | string | Header carrying the user's groups, comma separated | `X-Auth-Request-Groups` |
| `auth_groups` | map | Group (lowercase) → list of workspaces it may use | `{}` |
| `default_workspace` | string | Workspace everyone gets, and the one used when the cookie names a workspace the user may not use | `default` |
| `seo_enabled` | bool | Allow search engines/social previews to index and share the site: `/robots.txt` returns `Allow: /` instead of `Disallow: /`, and pages get a canonical link plus Open Graph/Twitter Card meta tags. Off by default so nothing is shared/indexed until explicitly opted in. | `false` |
| `semantic_search_enabled` | bool | Enable embedding-based "Semantic" search (the `/v1/semantic-search` endpoint, the "Semantic" option in the search dropdowns, and its entry on the `/api` page). When off, the option is hidden, the route is not served and the embedding model is never downloaded or loaded in memory. On by default so the feature keeps working unless explicitly disabled. | `true` |
| `health_score_enabled` | bool | Show the synthetic "Health" card (0-100 over security findings, human commit activity, "Required by" and Odoo series) on the module page. The detail cards stay either way; the `health` field is always in `/v1/module/*`. | `true` |

```yaml
# docker-compose.override.yaml
services:
  server:
    volumes:
      - ./server.yaml:/app/server.yaml
```
