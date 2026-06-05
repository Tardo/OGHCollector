CREATE TABLE IF NOT EXISTS author (
    id integer primary key autoincrement,
    name text not null unique
);

CREATE TABLE IF NOT EXISTS committer (
    id integer primary key autoincrement,
    name text not null unique
);

CREATE TABLE IF NOT EXISTS gh_organization (
    id integer primary key autoincrement,
    name text not null unique
);

CREATE TABLE IF NOT EXISTS gh_repository (
    id integer primary key autoincrement,
    name text unique not null,
    gh_organization_id integer not null references gh_organization(id),
    create_date text not null,
    update_date text not null,
    CONSTRAINT fk_gh_organization
        FOREIGN KEY (gh_organization_id)
        REFERENCES gh_organization(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_name_gh_organization_id ON gh_repository(name, gh_organization_id);

CREATE TABLE IF NOT EXISTS module (
    id integer primary key autoincrement,
    technical_name text not null,
    version_odoo integer not null,
    name text not null,
    version_module text not null,
    description text,
    website text,
    license text default 'LGPL-3',
    category text default 'Uncategorized',
    auto_install boolean not null default false,
    application boolean not null default false,
    installable boolean not null default true,
    gh_repository_id integer not null references gh_repository(id),
    create_date text not null,
    update_date text not null,
    folder_size integer not null,
    last_commit_hash text not null,
    last_commit_author text not null,
    last_commit_name text not null,
    last_commit_date text not null,
    last_commit_partof text,
    CONSTRAINT fk_gh_repository
        FOREIGN KEY (gh_repository_id)
        REFERENCES gh_repository(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_tech_name_ver_odoo_gh_repository_id ON module(technical_name, version_odoo, gh_repository_id);


CREATE TABLE IF NOT EXISTS dependency_type (
    id integer primary key autoincrement,
    name text not null unique
);
INSERT OR IGNORE INTO dependency_type(name) VALUES ('module'), ('python'), ('bin');

CREATE TABLE IF NOT EXISTS dependency (
    id integer primary key autoincrement,
    dependency_type_id integer not null references dependency_type(id),
    name text not null,
    CONSTRAINT fk_dependency_type
        FOREIGN KEY (dependency_type_id)
        REFERENCES dependency_type(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_type_name ON dependency(dependency_type_id, name);

CREATE TABLE IF NOT EXISTS dependency_module (
    id integer primary key autoincrement,
    dependency_id integer not null references dependency(id),
    module_id integer not null references module(id),
    CONSTRAINT fk_dependency
        FOREIGN KEY (dependency_id)
        REFERENCES dependency(id)
        ON DELETE CASCADE,
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_module ON dependency_module(dependency_id, module_id);

CREATE TABLE IF NOT EXISTS dependency_osv (
    id integer primary key autoincrement,
    dependency_module_id integer not null references dependency_module(id),
    osv_id text not null,
    details text not null,
    fixed_in text not null,
    CONSTRAINT fk_dependency_module_osv
        FOREIGN KEY (dependency_module_id)
        REFERENCES dependency_module(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_dep_dep_mod_id_osv_id ON dependency_osv(dependency_module_id, osv_id);

CREATE TABLE IF NOT EXISTS maintainer (
    id integer primary key autoincrement,
    name text not null unique
);

CREATE TABLE IF NOT EXISTS module_author (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    author_id integer not null references author(id),
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE,
    CONSTRAINT fk_author
        FOREIGN KEY (author_id)
        REFERENCES author(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_author ON module_author(module_id, author_id);

CREATE TABLE IF NOT EXISTS module_committer (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    committer_id integer not null references committer(id),
    commits integer not null,
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE,
    CONSTRAINT fk_committer
        FOREIGN KEY (committer_id)
        REFERENCES committer(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_committer ON module_committer(module_id, committer_id);

CREATE TABLE IF NOT EXISTS module_maintainer (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    maintainer_id integer not null references maintainer(id),
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE,
    CONSTRAINT fk_maintainer
        FOREIGN KEY (maintainer_id)
        REFERENCES maintainer(id)
        ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS uniq_module_maintainer ON module_maintainer(module_id, maintainer_id);

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

CREATE TABLE IF NOT EXISTS system_event_type (
    id integer primary key autoincrement,
    name text not null unique
);
INSERT OR IGNORE INTO system_event_type(name) VALUES 
    ('issue'), ('internal'), ('module'), ('maintainer'), 
    ('committer'), ('dependency'), ('author'), ('repository'), ('organization');

CREATE TABLE IF NOT EXISTS system_event (
    id integer primary key autoincrement,
    message text not null,
    date text not null,
    event_type_id integer not null references system_event_type(id),
    CONSTRAINT fk_event_type
        FOREIGN KEY (event_type_id)
        REFERENCES system_event_type(id)
        ON DELETE CASCADE
);
