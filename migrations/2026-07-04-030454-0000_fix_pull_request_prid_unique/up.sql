-- `prid` (PR/MR number) is only unique per repository, not globally: the same
-- number can appear in two different repos. Replace the bare UNIQUE with a
-- composite one. The table is empty in every environment (feature unshipped),
-- so a drop+recreate is safe and reaches every DB via the normal migration run.
DROP TABLE IF EXISTS pull_request;

CREATE TABLE IF NOT EXISTS pull_request (
    id integer primary key autoincrement,
    name text not null,
    version_odoo integer not null,
    module_technical_name text not null,
    prid integer not null,
    gh_repository_id integer not null references gh_repository(id),
    CONSTRAINT fk_gh_repository
        FOREIGN KEY (gh_repository_id)
        REFERENCES gh_repository(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_gh_repository_id_prid ON pull_request(gh_repository_id, prid);
