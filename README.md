<h1 align="center">
  <div>Odoo GitHub Collector</div>
</h1>

<p align="center">
A tool for collecting Odoo module metadata
</p>

This project is divided into two programs:
- oghcollector: The metadata collector
- oghserver: The web server to visualize data

---

## Requirements
1. Install Docker and Compose
2. Create a GitHub/GitLab Token:
  - https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens
3. Create token secrets ```gh_token``` or/and ```gl_token```,
    - ** Make sure that the file has only one line! (ex: nano -L gh_token.txt)
    ```yaml
    ...
        secrets:
          - gh_token

    secrets:
      gh_token:
        file: gh_token.txt
    ```

---

# OGHServer

## Start

_Before starting the server for the first time, you must launch OGHCollector to generate the database schema. This is necessary because OGHServer starts in read-only mode. After that, you can run it without any problems:_

```sh
docker compose up
```

## Configuration

You can add a volume to `/app/server.yaml` (you can use other formats like json if you prefer) to set your own configuration:

| Name | Type | Description | default |
| --- | --- | --- | --- |
| bind_address | string | The address to bind the server on | 0.0.0.0 |
| port | int | The port to bind the server on | 8080 |
| workers | int | The number of worker processes to run | 2 |
| template_autoreload | bool | Whether to automatically reload templates when they change | false |
| static_autoreload | bool | Whether to automatically reload static files when they change | false |
| allowed_origins | list of strings | A list of origins to allow | [] |
| timezone | string | The timezone to use | UTC |
| cookie_key | string | The key to use for the cookie | |
| upload_limit | int | The maximum bytes that can be uploaded | 2097152 |
| cache_ttl | int | The seconds that the cache is valid | 3600 |
| db_pool_max_size | int | The maximum number of connections that the pool can create and keep open at the same time | 15 |

# OGHCollector

## Usage

```sh
docker compose run --rm -u appuser -T app oghcollector <origin> <version> [git_type]
```

- `<origin>`:
  - The name of an organization (all repositories will be scanned).
  - The name of a repository (you can set the folders to be scanned separated by commas)
- `<version>`: The version of Odoo
- `[git_type]`: Optional. Git client to use (GL or GH). Default is GH.

### Examples

- Get Odoo modules in 18.0 (github):
 ```sh
 docker compose run --rm -u appuser -T app oghcollector odoo/odoo:/addons,/odoo/addons 18.0
 ```
- Get the OCA/web modules in 18.0 (github):
 ```sh
 docker compose run --rm -u appuser -T app oghcollector OCA/web 18.0
 ```
- Get all OCA modules in 18.0 (github):
 ```sh
 docker compose run --rm -u appuser -T app oghcollector OCA 18.0
 ```
- Get all MyGroup modules in 18.0 (gitlab):
 ```sh
 docker compose run --rm -u appuser -T app oghcollector MyGroup 18.0 GL:https://mygitlabinstance.com/api/v4/
 ```

** You may need to add ```-l traefik.enable=false```

## Auto Update

To auto-update the database you can create a CRON that invokes the `update_db.sh` script.
For example (update every 6 hours): 
```0 */6 * * * * * cd /path/to/OGHCollector && ./update_db.sh```.

# Extra Info

If you want to modify the configuration of `docker-compose.yaml`, it is recommended to create a new file `docker-compose.override.yaml` where to make the modifications.
