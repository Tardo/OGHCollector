-- Richer context for the module code analysis, aimed at making it directly
-- useful to an LLM (and eventually an MCP server) reading it: function
-- signatures/docstrings, all field/model keyword attributes (as JSON, since
-- Odoo field kwargs aren't a fixed enumerable set), and the view's root arch
-- tag (form/tree/kanban/qweb/...).
ALTER TABLE module_view ADD COLUMN view_type text;

ALTER TABLE module_model ADD COLUMN docstring text;
ALTER TABLE module_model ADD COLUMN attrs text;

ALTER TABLE module_model_field ADD COLUMN attrs text;

ALTER TABLE module_model_method ADD COLUMN signature text NOT NULL DEFAULT '';
ALTER TABLE module_model_method ADD COLUMN docstring text;
