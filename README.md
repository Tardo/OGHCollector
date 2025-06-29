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
2. Create a GitHub Token: https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens
3. Create the file `gh_token.txt` with the generated token in the root of this project folder,

# OGHServer

## Start

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

# OGHCollector

## Usage

```sh
docker compose exec -it -u appuser app oghcollector <origin> <version>
```

- `<origin>`:
  - The name of an organization (all repositories will be scanned).
  - The name of a repository (you can set the folders to be scanned separated by commas)
- `<version>`: The version of Odoo

### Examples

- Get Odoo modules in 18.0:
 ```sh
 docker compose exec -it -u appuser app oghcollector odoo/odoo:/addons,/odoo/addons 18.0
 ```
- Get the OCA/web modules in 18.0:
 ```sh
 docker compose exec -it -u appuser app oghcollector OCA/web 18.0
 ```
- Get all OCA modules in 18.0:
 ```sh
 docker compose exec -it -u appuser app oghcollector OCA 18.0
 ```
