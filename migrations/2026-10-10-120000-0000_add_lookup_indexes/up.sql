-- The only index here was (dependency_id, module_id), so every "what does
-- module X depend on" lookup - one per node of every dependency-closure walk
-- (module page, resolver, packs, license review, MCP) - scanned the whole
-- table.
CREATE INDEX idx_dependency_module_module_id ON dependency_module(module_id);
-- Atom feeds filter events by type, newest first.
CREATE INDEX idx_system_event_event_type_id ON system_event(event_type_id);
