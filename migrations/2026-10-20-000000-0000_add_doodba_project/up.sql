-- Doodba projects (repos generated from a doodba copier template, detected by
-- the collector's `doodba.rs`): one row per repository and Odoo version
-- (branch), rewritten by every job that collects it. Lists are JSON text:
-- read whole by the project pages, never queried by content. Every URL in
-- them has its credentials stripped (repos.yaml remotes carry tokens).
CREATE TABLE IF NOT EXISTS doodba_project (
    gh_repository_id INTEGER NOT NULL REFERENCES gh_repository(id) ON DELETE CASCADE,
    version_odoo INTEGER NOT NULL,
    last_commit_date TEXT NOT NULL,
    -- `.copier-answers.yml` `_src_path`/`_commit`; NULL without answers file.
    template_src TEXT,
    template_commit TEXT,
    template_commit_date TEXT,
    -- Template commits since `template_commit` on its default branch, NULL =
    -- unknown (template unreachable, or the commit isn't in its history).
    template_behind INTEGER,
    -- ["<hash> <subject>", ...] of those commits, newest first (capped).
    template_log TEXT NOT NULL,
    -- Allowlisted copier answers (odoo_version, postgres_version, ...).
    answers TEXT NOT NULL,
    base_image TEXT,
    -- repos.yaml: [{path, target, remotes: {name: url}, patches: [{remote, ref,
    -- state}]}], state = its PR/MR's open/closed/merged when it is one.
    repos TEXT NOT NULL,
    -- .pre-commit-config.yaml: [{repo, rev}]
    linters TEXT NOT NULL,
    -- odoo/custom/dependencies/*.txt: {"pip": [line, ...], ...}
    dependencies TEXT NOT NULL,
    -- ["<file> <host>", ...]: http(s) URLs with credentials in the project
    -- files (only where, never what).
    exposed TEXT NOT NULL DEFAULT '[]',
    -- PyPI advisories of the exact pip pins: [{package, version, id,
    -- fixed_in, summary}]
    advisories TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (gh_repository_id, version_odoo)
);

-- addons.yaml: the addons a project enables, per repos.yaml repo (`*` = all
-- of them). Its own table so "which projects use module X" is a join.
CREATE TABLE IF NOT EXISTS doodba_project_addon (
    gh_repository_id INTEGER NOT NULL REFERENCES gh_repository(id) ON DELETE CASCADE,
    version_odoo INTEGER NOT NULL,
    repo TEXT NOT NULL,
    technical_name TEXT NOT NULL,
    PRIMARY KEY (gh_repository_id, version_odoo, repo, technical_name)
);
CREATE INDEX IF NOT EXISTS doodba_project_addon_name
    ON doodba_project_addon (technical_name, version_odoo);
