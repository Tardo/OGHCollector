-- Fails if two organizations now have a repository with the same name.
-- foreign_keys: see up.sql.
PRAGMA foreign_keys = OFF;
BEGIN;
CREATE TABLE gh_repository_old (
    id integer primary key autoincrement,
    name text unique not null,
    gh_organization_id integer not null references gh_organization(id),
    create_date text not null,
    update_date text not null,
    url TEXT,
    CONSTRAINT fk_gh_organization
        FOREIGN KEY (gh_organization_id)
        REFERENCES gh_organization(id)
        ON DELETE CASCADE
);
INSERT INTO gh_repository_old (id, name, gh_organization_id, create_date, update_date, url)
    SELECT id, name, gh_organization_id, create_date, update_date, url FROM gh_repository;
UPDATE sqlite_sequence SET seq = (SELECT seq FROM sqlite_sequence WHERE name = 'gh_repository')
    WHERE name = 'gh_repository_old';
DROP TABLE gh_repository;
ALTER TABLE gh_repository_old RENAME TO gh_repository;
CREATE UNIQUE INDEX uniq_name_gh_organization_id ON gh_repository(name, gh_organization_id);
COMMIT;
