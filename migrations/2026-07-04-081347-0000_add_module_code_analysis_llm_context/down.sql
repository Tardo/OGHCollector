ALTER TABLE module_model_method DROP COLUMN docstring;
ALTER TABLE module_model_method DROP COLUMN signature;

ALTER TABLE module_model_field DROP COLUMN attrs;

ALTER TABLE module_model DROP COLUMN attrs;
ALTER TABLE module_model DROP COLUMN docstring;

ALTER TABLE module_view DROP COLUMN view_type;
