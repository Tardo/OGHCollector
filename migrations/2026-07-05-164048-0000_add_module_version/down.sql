ALTER TABLE module_model DROP COLUMN module_version_id;
ALTER TABLE module_view DROP COLUMN module_version_id;

DROP TABLE IF EXISTS module_version;
