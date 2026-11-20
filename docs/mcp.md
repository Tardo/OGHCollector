# OGHMcp

Streamable HTTP MCP endpoint exposing module and repository discovery, module metadata, documentation,
dependencies, code analysis, version history, version-to-version comparison, license review and
committer activity over the same data as `oghserver`.

## Configuration

Mount a volume to `/app/mcp.yaml` (JSON is also supported) to override the defaults. Every key can also be
set via an `OGHCOLLECTOR_MCP_`-prefixed environment variable (e.g. `OGHCOLLECTOR_MCP_CACHE_TTL`), which
takes precedence over the file:

| Name | Type | Description | Default |
| --- | --- | --- | --- |
| `cache_ttl` | int | Seconds cached module and repository query results stay valid | `3600` |

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

> Without `OGHCOLLECTOR_MCP_KEY_SECRET`, `oghmcp` has no authentication of its own (it logs a
> warning): if `oghserver` sits behind auth, set the key secret (see [Login with OIDC](auth.md#login-with-oidc))
> or keep `oghmcp` on a private network.
>
> `scan_instance` is disabled in MCP unless `OGHCOLLECTOR_SCAN_ENABLED=true`. Enable it only where
> access to the endpoint is controlled, as it makes outbound requests. Likewise
> `list_doodba_projects`/`get_doodba_project` need `OGHCOLLECTOR_PROJECTS_ENABLED=true`: they name
> your client projects.
