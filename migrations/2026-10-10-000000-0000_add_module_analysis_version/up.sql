-- Version of the collector's analysis rules that produced this module's
-- stored code analysis/findings (see collector::analyzer::ANALYSIS_VERSION).
-- 0 = never stamped: forces one re-analysis of every module, including ones
-- whose branch never gets another commit.
ALTER TABLE module ADD COLUMN analysis_version INTEGER NOT NULL DEFAULT 0;
