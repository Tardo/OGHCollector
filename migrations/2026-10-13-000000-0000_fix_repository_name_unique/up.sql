-- A repository name is only unique within its organization
-- (`uniq_name_gh_organization_id`), but the base table also had a bare UNIQUE
-- on `name`: collecting a repository named like one already stored under
-- another organization/group (a fork, `MyGroup/web` next to `OCA/web`)
-- crashed the whole collector job. SQLite can't drop a column constraint, so
-- the table is rebuilt with the same columns. Tables referencing it do so by
-- name, which resolves to the renamed copy.
-- foreign_keys must be OFF, or the DROP deletes every row first and cascades
-- to `module`, `pull_request`, ... It's OFF by default on the system SQLite the
-- binaries link, but ON in a bundled one (diesel CLI built with
-- `sqlite-bundled`), and it can only be switched outside a transaction: hence
-- `run_in_transaction = false` and the explicit BEGIN/COMMIT. It's left OFF,
-- what the binaries' connections already run with.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE gh_repository_new (
    id integer primary key autoincrement,
    name text not null,
    gh_organization_id integer not null references gh_organization(id),
    create_date text not null,
    update_date text not null,
    url TEXT,
    CONSTRAINT fk_gh_organization
        FOREIGN KEY (gh_organization_id)
        REFERENCES gh_organization(id)
        ON DELETE CASCADE
);
INSERT INTO gh_repository_new (id, name, gh_organization_id, create_date, update_date, url)
    SELECT id, name, gh_organization_id, create_date, update_date, url FROM gh_repository;
-- Keep the AUTOINCREMENT counter: ids are never reused, not even ones
-- orphaned rows might still point at.
UPDATE sqlite_sequence SET seq = (SELECT seq FROM sqlite_sequence WHERE name = 'gh_repository')
    WHERE name = 'gh_repository_new';
DROP TABLE gh_repository;
ALTER TABLE gh_repository_new RENAME TO gh_repository;
CREATE UNIQUE INDEX uniq_name_gh_organization_id ON gh_repository(name, gh_organization_id);
COMMIT;
