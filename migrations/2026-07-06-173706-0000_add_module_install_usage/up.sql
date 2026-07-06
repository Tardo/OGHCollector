-- Best-effort rendered readme fragments (readme/INSTALL.md, readme/USAGE.md),
-- same pattern as the existing description column (readme/DESCRIPTION.md):
-- lets the mcp crate answer "how do I install/use this module" straight from
-- what the module's own repo documents, without an LLM client cloning it.
ALTER TABLE module ADD COLUMN installation text;
ALTER TABLE module ADD COLUMN usage text;
