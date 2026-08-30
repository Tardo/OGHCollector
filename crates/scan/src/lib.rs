// Copyright Alexandre D. Díaz
//! Live scanner for an Odoo instance, shared between the MCP server and the
//! web server. Given a URL it probes a handful of well-known Odoo endpoints as
//! an HTTP client (never logging in), reads the response headers/body, and
//! turns that into a structured report with the critical points flagged.
//!
//! The scanner makes *outbound* requests from wherever it runs, so it carries
//! an SSRF guard (`validate_target`) that refuses to touch non-public
//! addresses - see `scan::validate_target`.
pub mod scan;

pub use scan::{
    scan_instance, CookieInfo, DatabaseInfo, Finding, ModuleInfo, ProbeResult, ScanReport, Timings,
    VersionInfo,
};
