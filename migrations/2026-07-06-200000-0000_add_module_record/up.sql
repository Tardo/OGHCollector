-- Every non-view record a module touches: security groups (res.groups),
-- record rules (ir.rule), cron jobs, access rights (from
-- ir.model.access.csv), demo/reference data, etc. Mirrors module_view
-- (delete+replace per module_version on every collector run), but generic
-- over `model` instead of being ir.ui.view-specific, and carries the
-- resolved `noupdate` flag so a caller can tell init-once data from data
-- that gets overwritten on every module upgrade.
CREATE TABLE IF NOT EXISTS module_record (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    xml_id text not null,
    model text not null,
    noupdate boolean not null default 0,
    fields text,
    module_version_id integer not null references module_version(id),
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_record_module_id ON module_record(module_id);
CREATE INDEX IF NOT EXISTS idx_module_record_module_version_id ON module_record(module_version_id);
CREATE INDEX IF NOT EXISTS idx_module_record_model ON module_record(model);
