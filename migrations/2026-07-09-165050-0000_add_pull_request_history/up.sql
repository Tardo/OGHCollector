-- Historical record of migration PRs/MRs that stopped being open, so the
-- modules page can show an "avg. time open before close" per Odoo version.
-- `pull_request` rows are deleted once a PR is no longer open (see
-- `pull_request::delete_outdated`), so without this table that duration is
-- lost forever. `closed_at` is when the collector *detected* the PR was gone,
-- not the provider's real close/merge timestamp (that would need an extra
-- API call per closed PR - not done here, see the `pull_request_history`
-- model doc for the accurate upgrade path). Both dates are required: a row
-- is only inserted when the closing PR had a known `created_at`.
CREATE TABLE IF NOT EXISTS pull_request_history (
    id integer primary key autoincrement,
    module_technical_name text not null,
    version_odoo integer not null,
    gh_repository_id integer not null references gh_repository(id),
    prid integer not null,
    created_at text not null,
    closed_at text not null,
    CONSTRAINT fk_gh_repository
        FOREIGN KEY (gh_repository_id)
        REFERENCES gh_repository(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_pull_request_history_version_odoo ON pull_request_history(version_odoo);
