# Workspaces and authentication

A workspace is a named view of the portal over some sources, e.g. your company's repositories next
to OCA and Odoo. Give a source `workspaces: [mycompany]` in `collector.yaml` (several names are
fine; entries with the same `source` share the union). Names are letters, digits, `-` and `_`.

- As soon as one repository has a workspace, the portal's top bar shows a selector (*All
  workspaces* by default). The choice is kept in a `workspace` cookie, per browser.
- Picking one limits the listings to that workspace's repositories **plus every repository without
  `workspaces`** (OCA, Odoo...): dashboard, modules, search (including semantic search), atlas,
  committers and committer pages, OSV, localization and the `/v1/search*` and `/v1/repository` API.
- Exception: **overview, modules and migration** list *only* the workspace's own repositories (the shared
  ones are left out), so you can keep what is yours apart from OCA/Odoo. A workspace with no
  repositories of its own (e.g. `default_workspace` under auth) shows the shared ones there instead
  of nothing. Migration still judges
  "exists at the other version" against every workspace.
- Module pages, dependency trees, license review, Doodba tools, badges, feeds, `/logs` and the
  sitemap always see everything, so a dependency on another workspace's module still resolves and
  its page opens. The MCP endpoint isn't filtered either.
- The workspaces are rewritten by every collector run of the source; a new one shows up in the
  selector after `cache_ttl`. A manual `oghcollector <source> <version> <git>` leaves them as
  they are; append `a,b` to change them, or `all` to show it in every workspace (same as a source
  without `workspaces`).

> A workspace is a filter, not access control: anyone can pick any workspace or open any module
> page. Keep an instance that collects private code behind authentication or a private network
> (see [Authentication](#authentication)).

## Authentication

The recommended way to provide API tokens is through Docker secrets, so they never end up in
`docker-compose.yaml` or shell history. `docker-compose.example.yaml` already wires them: create
`gh_token.txt` and `gl_token.txt` next to it (git-ignored; leave the one you don't use empty, since
compose refuses a missing file). The collector reads `/run/secrets/gh_token` and
`/run/secrets/gl_token`.

Git gets the token from a credential helper scoped to the repository's host, set up through
`GIT_CONFIG_*` environment variables. It's only used when the server asks for credentials, it's never
written to the clones' `.git/config` (in the data volume), and it never shows up in the process list.

> Collected private repositories are as visible as public ones: module metadata, code analysis and
> security findings show up on the dashboard, `/api/v1`, the MCP endpoint, feeds and badges. Keep an
> instance that collects private code behind authentication or a private network (both `server`
> and `mcp`).

Secret contents are trimmed, so a trailing newline is fine. A nonempty secret takes precedence;
an absent or empty secret falls back to the corresponding environment variable.

Alternatively, without secrets, set `OGHCOLLECTOR_TOKEN_GH` / `OGHCOLLECTOR_TOKEN_GL` as environment
variables (used as a fallback when the corresponding secret file isn't found).

## Login with OIDC

The apps don't talk to the identity provider. Traefik asks [oauth2-proxy](https://oauth2-proxy.github.io/oauth2-proxy/)
(which does the OIDC login) about every request; the proxy answers with the user's id and groups
as headers, and `oghserver` trusts them only when the request comes from `trusted_proxies`.

1. **`server.yaml`**

   ```yaml
   auth_enabled: true
   trusted_proxies: ["172.18.0.0/16"]   # oauth2-proxy / Traefik address or Docker subnet
   default_workspace: default           # what a user with no mapped group sees
   auth_groups:                         # OIDC group (lowercase) -> workspaces
     staff: [mycompany, oca]
   ```

   With `auth_enabled` the selector offers only the user's workspaces (plus `default_workspace`),
   a cookie naming another one falls back to `default_workspace`, and pages are sent
   `Cache-Control: private`. Identity headers from any other peer are dropped.

2. **oauth2-proxy** (example for GitLab as OIDC provider)

   ```
   --provider=oidc
   --oidc-issuer-url=https://gitlab.example.com
   --client-id=... --client-secret=...
   --redirect-url=https://ogh.example.com/oauth2/callback
   --scope="openid profile email"
   --oidc-groups-claim=groups
   --user-id-claim=sub              # needed by the MCP keys: the user header must be the numeric id
   --set-xauthrequest=true          # sends X-Auth-Request-User / -Groups
   --reverse-proxy=true
   --cookie-secret=...
   --email-domain=*
   --upstream=static://202          # auth only, it doesn't proxy
   ```

   Register the redirect URL above in the provider.

3. **Traefik**: a `forwardAuth` middleware on the server's router, and a route for `/oauth2/*`
   to oauth2-proxy.

   ```yaml
   traefik.http.middlewares.ogh-auth.forwardauth.address: http://oauth2-proxy:4180/oauth2/auth
   traefik.http.middlewares.ogh-auth.forwardauth.trustForwardHeader: "true"
   traefik.http.middlewares.ogh-auth.forwardauth.authResponseHeaders: X-Auth-Request-User,X-Auth-Request-Groups
   ```

4. **MCP keys** (optional). Don't put `/mcp` behind `forwardAuth`: MCP clients send
   `Authorization: Bearer <key>`. Set the same `OGHCOLLECTOR_MCP_KEY_SECRET` (or Docker secret
   `mcp_key_secret`) on `server` and `mcp`: the server's `/mcp` page then shows each logged-in user
   their key. `mcp` also needs `OGHCOLLECTOR_MCP_GITLAB_URL`, `OGHCOLLECTOR_MCP_GITLAB_GROUP` and a
   read-only `OGHCOLLECTOR_MCP_GITLAB_TOKEN` (or Docker secret `mcp_gitlab_token`): on every use
   (cached `OGHCOLLECTOR_MCP_GITLAB_CACHE` seconds, default 300) it checks that the user is still an
   active member of that group, so removing someone from the group revokes their key. Changing
   the secret revokes all keys.

> Authentication gates entry and filters listings; it is not per-workspace authorization. Any
> logged-in user can open any module page, and MCP is not filtered by workspace.
