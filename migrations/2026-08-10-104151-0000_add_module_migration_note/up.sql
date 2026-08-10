-- "Consider this before/after migrating to a newer Odoo version" findings
-- computed by the collector from a module's analyzed models/fields and raw
-- migration_facts (old-API base classes, hand-rolled SQL, deprecated
-- view/QWeb syntax, ...). Mirrors module_security_warning (delete+replace
-- per module_version on every collector run). `severity` is "warning" (will
-- likely break or needs an actual code change) or "info" (still works, but
-- worth a second look) - ordered with severity DESC ("warning" > "info"
-- alphabetically, unlike module_security_warning's "error"/"warning").
CREATE TABLE IF NOT EXISTS module_migration_note (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    severity text not null,
    code text not null,
    message text not null,
    context text,
    module_version_id integer not null references module_version(id),
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_migration_note_module_id ON module_migration_note(module_id);
CREATE INDEX IF NOT EXISTS idx_module_migration_note_module_version_id ON module_migration_note(module_version_id);
