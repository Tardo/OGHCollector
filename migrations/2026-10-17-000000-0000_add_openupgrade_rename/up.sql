-- Module renames/merges OpenUpgrade records per Odoo version (its
-- `apriori.py`): at `version_odoo`, `old_name` became `new_name`, an existing
-- module when `merged`. Upstream knowledge, not per organization: the
-- collector replaces a version's rows whenever it collects that version.
CREATE TABLE IF NOT EXISTS openupgrade_rename (
    version_odoo integer not null,
    old_name text not null,
    new_name text not null,
    merged boolean not null,
    primary key (version_odoo, old_name)
);
