DROP TABLE IF EXISTS pull_request;

CREATE TABLE IF NOT EXISTS pull_request (
    id integer primary key autoincrement,
    name text not null,
    version_odoo integer not null,
    module_technical_name text not null,
    prid integer not null unique,
    gh_repository_id integer not null references gh_repository(id),
    CONSTRAINT fk_gh_repository
        FOREIGN KEY (gh_repository_id)
        REFERENCES gh_repository(id)
        ON DELETE CASCADE
);
