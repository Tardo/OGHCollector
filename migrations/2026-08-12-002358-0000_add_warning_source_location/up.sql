-- Module-relative source file + 1-based line number the analyzer resolved a
-- security warning / migration note to (NULL when it couldn't resolve one -
-- e.g. the openerp_manifest fact, which is about a file's existence, not a
-- line). Lets the module page link straight to the offending source.
ALTER TABLE module_security_warning ADD COLUMN file TEXT;
ALTER TABLE module_security_warning ADD COLUMN line INTEGER;
ALTER TABLE module_migration_note ADD COLUMN file TEXT;
ALTER TABLE module_migration_note ADD COLUMN line INTEGER;
