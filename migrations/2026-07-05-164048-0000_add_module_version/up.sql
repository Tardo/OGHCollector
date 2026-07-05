-- Historize module versions: every distinct manifest `version` ever seen for
-- a module gets its own row here, and module_view/module_model snapshots hang
-- off that row instead of the module row directly, so re-analyzing a module
-- after a version bump no longer overwrites the previous version's snapshot.
-- module.version_module keeps meaning "the current version" (unchanged), so
-- "latest" is simply the module_version row whose version_module matches it
-- - no separate is_current flag needed.
CREATE TABLE IF NOT EXISTS module_version (
    id integer primary key autoincrement,
    module_id integer not null references module(id),
    version_module text not null,
    create_date text not null,
    update_date text not null,
    CONSTRAINT fk_module
        FOREIGN KEY (module_id)
        REFERENCES module(id)
        ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_module_version_module_id ON module_version(module_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_module_version_module_id_version_module ON module_version(module_id, version_module);

-- Backfill: today's schema only ever stored the current version, so this is
-- exactly one module_version row per existing module row.
INSERT INTO module_version (module_id, version_module, create_date, update_date)
SELECT id, version_module, create_date, update_date FROM module;

ALTER TABLE module_view ADD COLUMN module_version_id integer NOT NULL DEFAULT 0 references module_version(id);
ALTER TABLE module_model ADD COLUMN module_version_id integer NOT NULL DEFAULT 0 references module_version(id);

-- Deterministic because of the 1:1 backfill above; replaces the placeholder 0 default.
UPDATE module_view SET module_version_id = (
    SELECT mv.id FROM module_version mv WHERE mv.module_id = module_view.module_id
);
UPDATE module_model SET module_version_id = (
    SELECT mv.id FROM module_version mv WHERE mv.module_id = module_model.module_id
);

CREATE INDEX IF NOT EXISTS idx_module_view_module_version_id ON module_view(module_version_id);
CREATE INDEX IF NOT EXISTS idx_module_model_module_version_id ON module_model(module_version_id);
