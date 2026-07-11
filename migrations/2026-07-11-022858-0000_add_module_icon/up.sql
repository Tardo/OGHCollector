-- static/description/icon.png, base64-encoded, same pattern as the existing
-- description/installation/usage columns: lets the module page show it
-- without a separate binary-serving route.
ALTER TABLE module ADD COLUMN icon text;
