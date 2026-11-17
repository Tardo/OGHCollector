-- What the project's real instances answered when the collector queried them
-- (`collector.yaml` `instance:`): {"prod": Instance, "demo": Instance} (the
-- model's `Instances`). '{}' = never asked.
ALTER TABLE doodba_project ADD COLUMN instances TEXT NOT NULL DEFAULT '{}';
