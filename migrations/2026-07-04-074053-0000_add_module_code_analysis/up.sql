-- Module code analysis: which views a module touches, which models it defines
-- or extends, and the public methods/fields those models gain. Populated by
-- the collector's source analyzer (Python ast + XML parsing) alongside the
-- manifest info, and fully replaced on every collector run for a module.
CREATE TABLE IF NOT EXISTS module_view (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    xml_id text not null,
    name text,
    model text,
    inherit_xml_id text,
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_view_module_id ON module_view(module_id);

CREATE TABLE IF NOT EXISTS module_model (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    model_name text not null,
    class_name text not null,
    inherit_from text,
    is_new_model boolean not null default 0,
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_model_module_id ON module_model(module_id);
CREATE INDEX IF NOT EXISTS idx_module_model_model_name ON module_model(model_name);

CREATE TABLE IF NOT EXISTS module_model_field (
    id integer primary key autoincrement,
    module_model_id integer not null references module_model(id),
    name text not null,
    field_type text not null,
    relation text,
    CONSTRAINT fk_module_model
        FOREIGN KEY (module_model_id)
        REFERENCES module_model(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_model_field_module_model_id ON module_model_field(module_model_id);

CREATE TABLE IF NOT EXISTS module_model_method (
    id integer primary key autoincrement,
    module_model_id integer not null references module_model(id),
    name text not null,
    decorators text,
    CONSTRAINT fk_module_model
        FOREIGN KEY (module_model_id)
        REFERENCES module_model(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_model_method_module_model_id ON module_model_method(module_model_id);
