-- HTTP endpoints a module exposes (methods decorated with http.route),
-- extracted by the collector's source analyzer. Mirrors module_record
-- (delete+replace per module_version on every collector run). `routes` and
-- `methods` are JSON array text; `auth` is the resolved value (Odoo defaults
-- applied) or NULL for pure overrides of inherited routes; `csrf` is NULL
-- unless the decorator passes an explicit literal.
CREATE TABLE IF NOT EXISTS module_controller (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    class_name text not null,
    name text not null,
    routes text not null,
    auth text,
    http_type text not null,
    methods text,
    csrf boolean,
    website boolean not null default 0,
    uses_sudo boolean not null default 0,
    signature text not null default '',
    docstring text,
    module_version_id integer not null references module_version(id),
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_controller_module_id ON module_controller(module_id);
CREATE INDEX IF NOT EXISTS idx_module_controller_module_version_id ON module_controller(module_version_id);
