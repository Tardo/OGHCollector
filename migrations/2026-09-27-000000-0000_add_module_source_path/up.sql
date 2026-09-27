ALTER TABLE module ADD COLUMN source_path TEXT NOT NULL DEFAULT '';

-- Existing scans did not record the directory. Preserve OCA links and fix the
-- common Odoo layout until the collector next scans each branch and records
-- the exact path (including modules under odoo/addons or openerp/addons).
UPDATE module SET source_path = CASE
    WHEN gh_repository_id IN (
        SELECT r.id FROM gh_repository r
        JOIN gh_organization o ON o.id = r.gh_organization_id
        WHERE o.name = 'odoo' AND r.name = 'odoo'
    ) THEN CASE
        WHEN technical_name = 'base' AND version_odoo < 100 THEN 'openerp/addons/base'
        WHEN technical_name = 'base' THEN 'odoo/addons/base'
        ELSE 'addons/' || technical_name
    END
    ELSE technical_name
END;
