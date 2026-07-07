-- Security findings computed by the collector from a module's analyzed
-- records (ir.model.access rows, ir.rule domains, ...): overly lax
-- permissions, portal/public write access, privilege-escalation vectors.
-- Mirrors module_record (delete+replace per module_version on every
-- collector run). `severity` is "error" (grave: shown on the module detail
-- page) or "warning" (logged to system_event only).
CREATE TABLE IF NOT EXISTS module_security_warning (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    severity text not null,
    code text not null,
    message text not null,
    xml_id text,
    module_version_id integer not null references module_version(id),
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_security_warning_module_id ON module_security_warning(module_id);
CREATE INDEX IF NOT EXISTS idx_module_security_warning_module_version_id ON module_security_warning(module_version_id);
