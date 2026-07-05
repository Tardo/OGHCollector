-- system_event.message used to be a fully-baked HTML string with per-token CSS
-- classes assembled at insert time in Rust (see git history). That froze
-- presentation into stored history and made the data unsafe to render without
-- `| safe` in the template - a stored-XSS vector, since the values interpolated
-- into it (module/author/commit names) come from external git repositories.
--
-- Going forward, system_event.message is plain text rendered auto-escaped, and
-- `severity` drives styling instead of hand-picked span classes, so adding a
-- new kind of logged action no longer requires new markup/CSS. `is_html`
-- flags old rows so they keep rendering as before (with `| safe`) until they
-- roll off the current-month log view.
ALTER TABLE system_event ADD COLUMN severity text NOT NULL DEFAULT 'info';
ALTER TABLE system_event ADD COLUMN is_html boolean NOT NULL DEFAULT 1;

-- Best-effort backfill: 'issue' was only ever used for the "incorrect Odoo
-- version" warning, so it can be reclassified unambiguously. Other legacy
-- types (module/dependency/...) cover both additions and removals, so they
-- are left at the 'info' default.
UPDATE system_event SET severity = 'error'
WHERE event_type_id IN (SELECT id FROM system_event_type WHERE name = 'issue');
