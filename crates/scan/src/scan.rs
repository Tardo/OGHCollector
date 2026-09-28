// Copyright Alexandre D. Díaz
//! Live scanner for an Odoo instance. Given a URL it probes a handful of
//! well-known Odoo endpoints as an HTTP *client* (never logging in), reads the
//! response headers/body, and turns that into a structured report with the
//! critical points flagged. No authentication is used and nothing is written
//! anywhere - it is a pure, stateless read-only probe.
use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cached::{Cached, TimedSizedCache};
use lazy_static::lazy_static;
use regex::Regex;
use reqwest::header::HeaderValue;
use reqwest::redirect::Policy;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
    ClientConfig, DigitallySignedStruct, Error as RustlsError, RootCertStore, SignatureScheme,
};
use serde::Serialize;
use sqlitedb::models::module::ModuleRepositoryInfo;
use tokio_rustls::{TlsConnector, TlsStream};
use x509_parser::certificate::X509Certificate;
use x509_parser::extensions::GeneralName;
use x509_parser::parse_x509_certificate;
use x509_parser::x509::X509Name;

use serde_json::Value;
use url::Url;

const USER_AGENT: &str = concat!("OGHCollector-Scan/", env!("CARGO_PKG_VERSION"));

// Odoo versions still under bugfix support. The scanner has no live access to
// Odoo's support matrix, so this manual table is the only source of truth it
// has - bump it when a version leaves beta or an older one hits end-of-life.
// Standard support matrix as of September 2026; extended/vendor support differs.
const SUPPORTED_VERSIONS: &[&str] = &["18.0", "19.0", "20.0"];

// Per-request budget: a probe must answer within REQUEST_TIMEOUT and a
// connection must be established within CONNECT_TIMEOUT, otherwise it is
// recorded as a connection error rather than hanging the whole scan.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

// Bound memory and outbound work when scanning a hostile public target.
const MAX_TARGET_LEN: usize = 2048;
const MAX_RESPONSE_BODY: usize = 512 * 1024;
const MAX_RESOLVED_ADDRS: usize = 8;
const MAX_MODULES: usize = 200;
// ponytail: global cap; use authenticated per-client limits if this is exposed publicly.
const MAX_CONCURRENT_SCANS: usize = 2;
const MAX_CONCURRENT_PROBES: usize = 4;

// Scan cache: a previous report of the same normalized host is reused for
// SCAN_CACHE_TTL before re-probing, so repeated scans of one instance (the
// dashboard, the MCP tool, a cron) do not hammer it. The cache keys on the
// *normalized* URL but restores the caller's original `target_url` on return.
const SCAN_CACHE_SIZE: usize = 100;
const SCAN_CACHE_TTL: Duration = Duration::from_secs(300);

lazy_static! {
    static ref SCAN_CACHE: Mutex<TimedSizedCache<String, ScanReport>> = Mutex::new(
        TimedSizedCache::with_size_and_lifespan(SCAN_CACHE_SIZE, SCAN_CACHE_TTL),
    );
    static ref SCAN_PERMITS: tokio::sync::Semaphore =
        tokio::sync::Semaphore::const_new(MAX_CONCURRENT_SCANS);
    // The public /website/info page carries Odoo's own version marker: a
    // `data-odoo-vsn="18.0"` attribute and/or an "Odoo Version 18.1" label.
    static ref VERSION_ATTR_RE: Regex =
        Regex::new(r#"data-odoo-vsn="(\d+\.\d+)"#).expect("valid version attr regex");
    static ref VERSION_LABEL_RE: Regex =
        Regex::new(r"Odoo Version\s+(\d+\.\d+)").expect("valid version label regex");
}

/// Top-level report returned by the `scan_instance` tool. Every field is
/// populated even on failure (with `reachable: false` and a populated
/// `findings`), so a caller always gets a well-formed document back.
#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    /// The URL exactly as passed to the tool.
    pub target_url: String,
    /// False when no probe ever got an HTTP response (DNS/connect/timeout).
    pub reachable: bool,
    /// Connection-level error, when the instance could not be reached at all.
    pub error: Option<String>,
    /// Odoo version, when it could be detected from a header or the manifest.
    pub version: VersionInfo,
    /// Whether the probe target was resolved over HTTPS.
    pub is_https: bool,
    /// Whether debug mode appears to be enabled on the instance.
    pub debug_mode: bool,
    /// Timing figures (see `Timings`).
    pub timings: Timings,
    /// Database-manager / database-enumeration exposure.
    pub database: DatabaseInfo,
    /// Installed-module enumeration (best effort - see `analyze_modules`).
    pub modules: ModuleInfo,
    /// Session cookies seen, with their security flags parsed out.
    pub cookies: Vec<CookieInfo>,
    /// Per-endpoint probe results, in probe order.
    pub endpoints: Vec<ProbeResult>,
    /// The richest endpoint's sanitized headers (used for the header findings).
    pub main_endpoint: Option<ProbeResult>,
    /// Server response header value, when present.
    pub server_header: Option<String>,
    /// Whether the probe target looks like an Odoo web application. When false,
    /// module enumeration and version checks are skipped (see `is_odoo`).
    pub is_odoo: bool,
    /// The peer TLS certificate, when the target is HTTPS and a certificate was
    /// presented (even if the chain is untrusted - see `verified`).
    pub certificate: Option<CertificateInfo>,
    /// Review findings with stable rule codes and severity.
    pub findings: Vec<Finding>,
    /// The pass/fail result of each security check that ran.
    pub checks: Vec<SecurityCheck>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VersionInfo {
    /// The raw version string Odoo reported (e.g. "18.0.12"), if any.
    pub detected: Option<String>,
    /// Where the version came from: "header", "manifest", or "none".
    pub source: &'static str,
    /// Whether `detected` is in `SUPPORTED_VERSIONS`.
    pub supported: bool,
    /// "supported", "outdated" or "unknown".
    pub status: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Timings {
    /// Total wall-clock time of the whole scan.
    pub total_ms: f64,
    /// Time-to-first-byte of the main `/web` probe.
    pub main_ttfb_ms: f64,
    /// Total time of the main `/web` probe.
    pub main_total_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct DatabaseInfo {
    /// True when `/web/database/info` answered 200 and listed databases.
    pub info_available: bool,
    /// HTTP status of `/web/database/info`, when probed.
    pub info_status: Option<u16>,
    /// Database names returned by `/web/database/info`, when exposed.
    pub databases: Vec<String>,
    /// True when a 2xx response contains database-management form paths.
    pub manager_available: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleInfo {
    /// HTTP status of the `/web/modules` probe, when probed.
    pub page_status: Option<u16>,
    /// Content type of the `/web/modules` response, when available.
    pub page_content_type: Option<String>,
    /// Module technical names extracted from the response (best effort).
    pub module_names: Vec<String>,
    /// Number of module names extracted.
    pub module_count: usize,
    /// True when enumeration stopped at `MAX_MODULES` to keep a hostile page
    /// from inflating the report.
    pub truncated: bool,
    /// Installed modules that exist in the collector DB (org/repo), so the UI
    /// can link each one to its info page. Read-only; never written here.
    pub module_links: Vec<ModuleRepositoryInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CookieInfo {
    pub name: String,
    pub secure: bool,
    pub httponly: bool,
    pub samesite: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ProbeResult {
    pub url: String,
    pub status_code: u16,
    pub status_text: String,
    pub content_type: Option<String>,
    pub body_size: usize,
    pub timing_ms: f64,
    pub ttfb_ms: f64,
    /// For a 3xx response, the `Location` header.
    pub redirect_location: Option<String>,
    /// Sanitized headers (cookie values redacted).
    pub headers: BTreeMap<String, String>,
    /// Recognized sensitive-file format; contents/secrets are never serialized.
    pub sensitive_file_signature: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub severity: String,
    pub code: String,
    pub message: String,
}

/// A single security check that ran against the instance.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SecurityCheck {
    pub code: String,
    /// `None` means this check could not be assessed from an anonymous probe.
    pub passed: Option<bool>,
    pub detail: String,
}

/// Information extracted from the peer TLS certificate of an HTTPS target.
/// `verified` is true only when the chain built up to a trusted root; a
/// self-signed or otherwise untrusted certificate is still reported with
/// `verified: false` so the operator can see exactly what the server presents.
#[derive(Debug, Clone, Serialize)]
pub struct CertificateInfo {
    /// Subject common name (CN), when present.
    pub subject_cn: Option<String>,
    /// Subject organization (O), when present.
    pub subject_org: Option<String>,
    /// Issuer common name (CN), when present.
    pub issuer_cn: Option<String>,
    /// Issuer organization (O), when present.
    pub issuer_org: Option<String>,
    /// Not-before date, as reported by the certificate.
    pub not_before: Option<String>,
    /// Not-after date, as reported by the certificate.
    pub not_after: Option<String>,
    /// True when the subject and issuer distinguished names are identical.
    pub self_signed: bool,
    /// True when the chain verified against the Mozilla root store.
    pub verified: bool,
    /// True when the leaf certificate is currently within its validity window.
    pub valid_now: bool,
    /// DNS subject-alternative names, when the certificate carries any.
    pub sans: Vec<String>,
}

impl Finding {
    fn new(severity: &str, code: impl Into<String>, message: impl Into<String>) -> Finding {
        Finding {
            severity: severity.to_string(),
            code: code.into(),
            message: message.into(),
        }
    }
}

/// A single probe: the public `ProbeResult` plus the raw body and the raw
/// `Set-Cookie` value, which are needed to extract the version, databases,
/// modules and cookie flags but must not be serialized into the report.
#[derive(Clone)]
struct Probe {
    result: ProbeResult,
    body: String,
    raw_set_cookie: Option<String>,
    error: Option<String>,
}

impl Probe {
    fn ok(result: ProbeResult, body: String, raw_set_cookie: Option<String>) -> Probe {
        Probe {
            result,
            body,
            raw_set_cookie,
            error: None,
        }
    }
}

/// Entry point for the `scan_instance` tool. Never returns an `Err` - a
/// connection failure is itself part of the report (`reachable: false`).
pub async fn scan_instance(target: &str) -> ScanReport {
    if target.len() > MAX_TARGET_LEN {
        return ScanReport::fatal(target, "URL is too long.");
    }
    let normalized = match normalize_url(target) {
        Ok(url) => url,
        Err(e) => return ScanReport::fatal(target, &format!("invalid URL '{target}': {e}")),
    };

    // Fast path: a previous scan of this exact host is still fresh. The report
    // is returned with the original `target` restored as `target_url` (the cache
    // keys on the normalized URL), so callers always see what they asked for.
    if let Some(report) = cached_report(&normalized, target) {
        return report;
    }

    let permit = match SCAN_PERMITS.try_acquire() {
        Ok(permit) => permit,
        Err(_) => return ScanReport::limited(target),
    };
    let scan_target = match resolve_target(&normalized).await {
        Ok(scan_target) => scan_target,
        Err(error) => {
            return ScanReport::fatal(target, &format!("invalid URL '{target}': {error}"))
        }
    };
    let is_https = scan_target.normalized.starts_with("https://");
    let scan_started = Instant::now();

    // The TLS probe runs concurrently with the HTTP probes for HTTPS targets;
    // it is spawned so a slow handshake never blocks the HTTP probes.
    let cert_handle = if is_https {
        let host = scan_target.host.clone();
        let addrs = scan_target.addrs.clone();
        Some(tokio::spawn(async move {
            tokio::time::timeout(REQUEST_TIMEOUT, probe_certificate(host, addrs))
                .await
                .ok()
                .flatten()
        }))
    } else {
        None
    };

    let client = match reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .redirect(Policy::none())
        // Connect only to the public addresses resolved and checked above. This
        // prevents a hostname from rebinding to an internal address afterwards.
        .resolve_to_addrs(&scan_target.host, &scan_target.addrs)
        .build()
    {
        Ok(c) => c,
        Err(e) => return ScanReport::fatal(target, &format!("failed to build HTTP client: {e}")),
    };

    let urls = probe_urls(&scan_target.normalized);
    let mut probes = Vec::with_capacity(urls.len());
    for urls in urls.chunks(MAX_CONCURRENT_PROBES) {
        let mut handles = Vec::with_capacity(urls.len());
        for url in urls {
            let url = url.clone();
            let client = client.clone();
            handles.push(tokio::spawn(async move { probe_one(&client, &url).await }));
        }
        for handle in handles {
            probes.push(handle.await.expect("probe task panicked"));
        }
    }

    let reachable = probes.iter().any(|p| p.result.status_code >= 100);
    let error = if reachable {
        None
    } else {
        probes.iter().find_map(|p| p.error.clone())
    };

    let main = pick_main(&probes);
    let (cookies, debug_mode) = analyze_session(&probes);
    let database = analyze_database(&probes);
    let server_header = main
        .as_ref()
        .and_then(|p| p.result.headers.get("server").cloned());

    // Await the concurrent TLS probe (if any). A failure to complete the
    // handshake - or a panic in the task - leaves `certificate` as `None`; the
    // HTTP probes still ran and the report is complete.
    let cert = match cert_handle {
        Some(handle) => handle.await.ok().flatten(),
        None => None,
    };

    let total_ms = scan_started.elapsed().as_secs_f64() * 1000.0;
    let timings = match &main {
        Some(p) => Timings {
            main_ttfb_ms: p.result.ttfb_ms,
            main_total_ms: p.result.timing_ms,
            total_ms,
        },
        None => Timings {
            total_ms,
            main_ttfb_ms: 0.0,
            main_total_ms: 0.0,
        },
    };

    let mut report = ScanReport {
        target_url: target.to_string(),
        reachable,
        error,
        version: VersionInfo {
            detected: None,
            source: "none",
            supported: false,
            status: "unknown",
        },
        is_https,
        debug_mode,
        timings,
        database,
        modules: ModuleInfo {
            page_status: None,
            page_content_type: None,
            module_names: Vec::new(),
            module_count: 0,
            truncated: false,
            module_links: Vec::new(),
        },
        cookies,
        endpoints: probes.iter().map(|p| p.result.clone()).collect(),
        main_endpoint: main.map(|p| p.result),
        server_header,
        is_odoo: false,
        certificate: cert,
        findings: Vec::new(),
        checks: Vec::new(),
    };
    // Module enumeration and version assessment only make sense for an
    // Odoo web application. Decide that once, up front, from the live probes.
    report.is_odoo = is_odoo(&probes);
    if report.is_odoo {
        report.modules = analyze_modules(&probes);
    }
    report.version = detect_version(&probes);
    report.modules.module_links = infer_module_links(
        &report.modules.module_names,
        report.version.detected.as_deref(),
    );
    report.findings = build_findings(&report);
    report.checks = collect_checks(&report);

    // Store the result so a repeated scan of the same host is served from cache.
    store_report(&normalized, report.clone());
    drop(permit);
    report
}

/// A report for an instance that could never be probed (bad URL, client build
/// failure). Used both for the fatal paths above and as the shape everything
/// else fills in.
impl ScanReport {
    fn fatal(target: &str, error: &str) -> ScanReport {
        ScanReport {
            target_url: target.to_string(),
            reachable: false,
            error: Some(error.to_string()),
            version: VersionInfo {
                detected: None,
                source: "none",
                supported: false,
                status: "unknown",
            },
            is_https: false,
            debug_mode: false,
            timings: Timings {
                total_ms: 0.0,
                main_ttfb_ms: 0.0,
                main_total_ms: 0.0,
            },
            database: DatabaseInfo {
                info_available: false,
                info_status: None,
                databases: Vec::new(),
                manager_available: false,
            },
            modules: ModuleInfo {
                page_status: None,
                page_content_type: None,
                module_names: Vec::new(),
                module_count: 0,
                truncated: false,
                module_links: Vec::new(),
            },
            cookies: Vec::new(),
            endpoints: Vec::new(),
            main_endpoint: None,
            server_header: None,
            is_odoo: false,
            certificate: None,
            findings: vec![Finding::new(
                "info",
                "unreachable",
                format!("Instance could not be scanned: {error}"),
            )],
            checks: vec![SecurityCheck {
                code: "unreachable".into(),
                passed: None,
                detail: "No probe received an HTTP response.".into(),
            }],
        }
    }

    fn limited(target: &str) -> ScanReport {
        let mut report = Self::fatal(target, "Scanner is busy. Please retry shortly.");
        report.findings[0].severity = "info".into();
        report.findings[0].code = "scan-limited".into();
        report.checks[0] = SecurityCheck {
            code: "scan-availability".into(),
            passed: None,
            detail: "Scanner capacity is in use; retry shortly.".into(),
        };
        report
    }
}

/// The target with its public DNS result pinned for every outbound connection.
struct ScanTarget {
    normalized: String,
    host: String,
    addrs: Vec<SocketAddr>,
}

/// Turn `target` into an origin-only probe base. The scanner deliberately does
/// not accept paths, queries or credentials: every probe is a fixed public
/// endpoint below the submitted origin.
fn normalize_url(target: &str) -> Result<String, String> {
    let trimmed = target.trim();
    if trimmed.is_empty() {
        return Err("empty URL".to_string());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    let url = Url::parse(&with_scheme).map_err(|e| e.to_string())?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(format!(
            "only http(s) URLs are allowed, got '{}'",
            url.scheme()
        ));
    }
    if url.host_str().is_none() {
        return Err("URL has no host".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("URL credentials are not allowed".to_string());
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(
            "URL must contain only an origin, without a path, query or fragment".to_string(),
        );
    }
    Ok(url.origin().ascii_serialization())
}

/// SSRF guard: refuse to scan anything other than a public http(s) resource.
/// `scan_instance` runs as an *outbound* client from wherever the server runs,
/// so a caller pointing it at `http://169.254.169.254/` (cloud metadata),
/// `http://127.0.0.1/` or `http://192.168.x.x/` would make the server reach
/// internal services. Fail-closed: an explicit non-http(s) scheme, an
/// unresolvable host or any non-public resolved address is rejected.
///
/// `normalize_url` accepts a bare host as HTTPS but rejects explicit non-HTTP
/// schemes, credentials and non-origin URL components before resolution.
pub async fn validate_target(target: &str) -> Result<String, String> {
    if target.len() > MAX_TARGET_LEN {
        return Err("URL is too long".to_string());
    }
    let normalized = normalize_url(target)?;
    resolve_target(&normalized)
        .await
        .map(|target| target.normalized)
}

/// Resolve a normalized target once and retain only the checked public socket
/// addresses. `reqwest` and the certificate probe both consume this result,
/// closing the DNS-rebinding gap between validation and connection.
async fn resolve_target(normalized: &str) -> Result<ScanTarget, String> {
    let url = Url::parse(normalized).map_err(|e| format!("invalid URL '{normalized}': {e}"))?;
    let host = url
        .host_str()
        .ok_or_else(|| "URL has no host".to_string())?
        .to_string();
    let port = url
        .port_or_known_default()
        .ok_or_else(|| "URL has no usable port".to_string())?;
    // An IP literal (optionally an IPv6 in brackets) is checked directly;
    // anything else is a hostname that must be resolved. Fail-closed: if
    // resolution fails, reject rather than letting reqwest fall back to a
    // bogus address.
    let ips: Vec<IpAddr> = if let Ok(ip) = host.parse::<IpAddr>() {
        vec![ip]
    } else {
        tokio::net::lookup_host((host.as_str(), port))
            .await
            .map_err(|e| format!("could not resolve host '{host}': {e}"))?
            .map(|sa| sa.ip())
            .collect()
    };
    if ips.is_empty() {
        return Err(format!("could not resolve host '{host}'"));
    }
    let mut addrs = BTreeSet::new();
    for ip in ips {
        if is_blocked_ip(ip) {
            return Err(format!(
                "refusing to scan non-public address '{ip}' (private, loopback, link-local, \
                 reserved, multicast or broadcast)"
            ));
        }
        addrs.insert(SocketAddr::new(ip, port));
    }
    let addrs: Vec<_> = addrs.into_iter().take(MAX_RESOLVED_ADDRS).collect();
    Ok(ScanTarget {
        normalized: normalized.to_string(),
        host,
        addrs,
    })
}

/// Whether an address must never be scanned. Matches the "bad" categories
/// explicitly (fail-closed) rather than trusting an `is_global()` negation,
/// which can miss ranges as the threat model grows.
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [first, second, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_documentation()
                || v4.is_broadcast()
                || first == 0
                // Shared address space (RFC 6598) and benchmarking space
                // (RFC 2544) are not public scan targets.
                || (first == 100 && (64..=127).contains(&second))
                || (first == 198 && (second == 18 || second == 19))
                || (first == 192 && second == 0)
                || first >= 240
        }
        IpAddr::V6(v6) => {
            v6.to_ipv4_mapped().is_some_and(is_blocked_ipv4)
                || v6.is_loopback()
                || v6.is_unique_local()
                || v6.is_unicast_link_local()
                || v6.is_unspecified()
                || v6.is_multicast()
                // Documentation and discard-only IPv6 space must never leave
                // the scanner even if a resolver returns it.
                || (v6.segments()[0] == 0x2001 && v6.segments()[1] == 0x0db8)
                || (v6.segments()[0] == 0x0100
                    && v6.segments()[1] == 0
                    && v6.segments()[2] == 0
                    && v6.segments()[3] == 0)
        }
    }
}

fn is_blocked_ipv4(v4: std::net::Ipv4Addr) -> bool {
    is_blocked_ip(IpAddr::V4(v4))
}

/// The endpoints probed, in order. `/web` is the main entry (version header,
/// session cookie, security headers); the `/database/*` probes catch the
/// enumeration exposure; `/session` and `/modules` are best-effort; the
/// manifest carries the version as a fallback when the header is absent.
///
/// `/website/info` is the public Odoo page that lists installed apps/localizations
/// without auth (controller `website_info`, template `website.website_info`); it
/// is the primary module source, with the JSON manifest as fallback.
fn probe_urls(base: &str) -> Vec<String> {
    let mut urls = vec![
        format!("{base}/"),
        format!("{base}/web"),
        format!("{base}/web/database/info"),
        format!("{base}/web/database/manager"),
        format!("{base}/web/session"),
        format!("{base}/web/modules"),
        format!("{base}/website/info"),
        format!("{base}/web/static/manifest.json"),
    ];
    for (path, _, _) in SENSITIVE_PATHS {
        urls.push(format!("{base}{path}"));
    }
    urls
}

/// Return a cached report for `normalized`, refreshing its TTL, or `None` when
/// the entry is missing or expired. The original `target` is restored as
/// `target_url` so the caller sees what it asked for, not the normalized form.
fn cached_report(normalized: &str, target: &str) -> Option<ScanReport> {
    let mut cache = SCAN_CACHE.lock().ok()?;
    let hit = cache.cache_get(normalized).cloned();
    drop(cache);
    match hit {
        Some(mut report) => {
            report.target_url = target.to_string();
            store_report(normalized, report.clone());
            Some(report)
        }
        None => None,
    }
}

/// Insert `report` into the scan cache under `normalized`. A poisoned lock or a
/// full cache simply drops the write - the scan still completes.
fn store_report(normalized: &str, report: ScanReport) {
    if let Ok(mut cache) = SCAN_CACHE.lock() {
        cache.cache_set(normalized.to_string(), report);
    }
}

/// Whether a probe target looks like an Odoo web application. Checks the
/// `x-openerp-version` header, the `/web/static/manifest.json` shape, and - as
/// the catch-all - the public `/website/info` page markers. Returns false for
/// anything that does not answer, so a dead host is never mistaken for Odoo.
fn is_odoo(probes: &[Probe]) -> bool {
    // The gold standard: the version header Odoo sets on virtually every
    // response. Present on the main endpoint or any probe. Odoo renamed it from
    // `x-openerp-version` to `x-odoo-version` in 17.0, so accept both.
    if probes.iter().any(|p| {
        p.result.headers.contains_key("x-openerp-version")
            || p.result.headers.contains_key("x-odoo-version")
    }) {
        return true;
    }
    // The manifest.json shape: an Odoo app manifest with a "modules" object.
    if let Some(p) = probes
        .iter()
        .find(|p| p.result.url.ends_with("manifest.json"))
    {
        if is_odoo_manifest(&p.body) {
            return true;
        }
    }
    // Any response referencing the web.assets_frontend bundle in its <head>:
    // Odoo's frontend asset bundle renders on virtually every page, so this
    // catches instances whose version header was stripped by a proxy.
    if has_odoo_assets_frontend(probes) {
        return true;
    }
    // The public "/website/info" page: its headings are the catch-all that
    // catches Odoo even when the header and manifest are both absent.
    has_odoo_page_markers(probes)
}

/// Whether any probe's HTML references the `web.assets_frontend` bundle in its
/// `<head>`. It is Odoo's frontend asset bundle and renders on virtually every
/// page, so its presence is a strong, version-independent signal - useful when
/// the version header has been stripped by a proxy in front of the instance.
fn has_odoo_assets_frontend(probes: &[Probe]) -> bool {
    probes
        .iter()
        .any(|p| p.body.contains("web.assets_frontend"))
}

/// Whether a `manifest.json` body is an Odoo app manifest: a JSON object with
/// a "modules" object whose values are themselves objects.
fn is_odoo_manifest(body: &str) -> bool {
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body.trim()) else {
        return false;
    };
    matches!(map.get("modules"), Some(Value::Object(obj)) if !obj.is_empty())
}

/// Whether the public `/website/info` probe returned an Odoo page. The page
/// always renders "Installed Applications" plus one of the localization/chart
/// sections, so requiring both is a strong, Odoo-specific signal.
fn has_odoo_page_markers(probes: &[Probe]) -> bool {
    let Some(p) = probes
        .iter()
        .find(|p| p.result.url.ends_with("/website/info"))
    else {
        return false;
    };
    if p.result.status_code < 200 || p.result.status_code >= 400 {
        return false;
    }
    let body = p.body.to_lowercase();
    body.contains("installed applications") && body.contains("odoo version")
}

/// Parse the technical names of installed modules from a `/website/info` page.
/// Returns an empty list if the page is not an Odoo module list.
///
/// Odoo renders each module as `<dd class="text-muted">Technical name: <name>,
/// author: <author></dd>`, so the technical name is the text between
/// "Technical name:" and ", author:".
fn parse_module_names_from_page(body: &str) -> (Vec<String>, bool) {
    let re = match Regex::new(r#"<dd[^>]*>\s*Technical name:\s*([^,<>]+?)\s*,\s*author:"#) {
        Ok(r) => r,
        Err(_) => return (Vec::new(), false),
    };
    limit_module_names(re.captures_iter(body).filter_map(|c| {
        let name = c[1].trim();
        if is_valid_module_name(name) {
            Some(name.to_string())
        } else {
            None
        }
    }))
}

fn limit_module_names(candidates: impl Iterator<Item = String>) -> (Vec<String>, bool) {
    let mut names = Vec::new();
    let mut seen = BTreeSet::new();
    for name in candidates {
        if !is_valid_module_name(&name) {
            continue;
        }
        if !seen.insert(name.clone()) {
            continue;
        }
        if names.len() == MAX_MODULES {
            return (names, true);
        }
        names.push(name);
    }
    (names, false)
}

/// A technical name is a module identifier: it starts with a letter (or an
/// `l10n_` localization prefix) and contains only lowercase letters, digits and
/// underscores.
fn is_valid_module_name(name: &str) -> bool {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 {
        return false;
    }
    let rest = name.strip_prefix("l10n_").unwrap_or(name);
    rest.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && matches!(rest.chars().next(), Some(c) if c.is_ascii_lowercase())
}

/// Probe the TLS certificate presented by the already-validated addresses. First attempt verifies
/// against the Mozilla root store; a success means the chain is trusted. The
/// second attempt accepts any certificate so a broken chain (self-signed,
/// expired, untrusted issuer) still yields the peer's certificate.
async fn probe_certificate(host: String, addrs: Vec<SocketAddr>) -> Option<CertificateInfo> {
    let name = ServerName::try_from(host.clone()).ok()?;
    for addr in addrs {
        if let Some(stream) = connect_tcp(addr).await {
            if let Some(tls) = connect_tls(&VERIFY_CONNECTOR, name.clone(), stream).await {
                return extract_cert(&tls, true);
            }
        }
        // The chain failed verification; reconnect to the same checked address
        // accepting any certificate so it can still be reported as untrusted.
        if let Some(stream) = connect_tcp(addr).await {
            if let Some(tls) = connect_tls(&FALLBACK_CONNECTOR, name.clone(), stream).await {
                return extract_cert(&tls, false);
            }
        }
    }
    None
}

async fn connect_tcp(addr: SocketAddr) -> Option<tokio::net::TcpStream> {
    match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => Some(stream),
        _ => None,
    }
}

/// Complete a TLS handshake against `name`, bounded by CONNECT_TIMEOUT. Returns
/// the stream on success, or `None` on a failed/expired handshake.
async fn connect_tls(
    connector: &TlsConnector,
    name: ServerName<'static>,
    stream: tokio::net::TcpStream,
) -> Option<TlsStream<tokio::net::TcpStream>> {
    match tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(name, stream)).await {
        Ok(Ok(tls)) => Some(TlsStream::Client(tls)),
        _ => None,
    }
}

/// Pull the peer certificate out of a completed handshake and parse it.
fn extract_cert(tls: &TlsStream<tokio::net::TcpStream>, verified: bool) -> Option<CertificateInfo> {
    let (_stream, conn) = tls.get_ref();
    let certs = conn.peer_certificates()?;
    let der = certs.first()?;
    let (_, cert) = parse_x509_certificate(der.as_ref()).ok()?;
    Some(build_cert_info(&cert, verified))
}

/// Build the public certificate view from a parsed X.509 certificate.
fn build_cert_info(cert: &X509Certificate<'_>, verified: bool) -> CertificateInfo {
    let mut sans = Vec::new();
    if let Ok(Some(ext)) = cert.subject_alternative_name() {
        for name in ext.value.general_names.iter() {
            if let GeneralName::DNSName(dns) = name {
                sans.push(dns.to_string());
            }
        }
    }
    CertificateInfo {
        subject_cn: name_cn(cert.subject()),
        subject_org: name_org(cert.subject()),
        issuer_cn: name_cn(cert.issuer()),
        issuer_org: name_org(cert.issuer()),
        not_before: Some(cert.validity().not_before.to_string()),
        not_after: Some(cert.validity().not_after.to_string()),
        self_signed: cert.subject().as_raw() == cert.issuer().as_raw(),
        verified,
        valid_now: cert.validity().is_valid(),
        sans,
    }
}

/// The first common name (CN) of a distinguished name, when present.
fn name_cn(name: &X509Name<'_>) -> Option<String> {
    name.iter_common_name()
        .next()
        .and_then(|a| a.as_str().ok())
        .map(str::to_string)
}

/// The first organization (O) of a distinguished name, when present.
fn name_org(name: &X509Name<'_>) -> Option<String> {
    name.iter_organization()
        .next()
        .and_then(|a| a.as_str().ok())
        .map(str::to_string)
}

/// A rustls client that verifies against the Mozilla root store.
fn client_verify_config() -> ClientConfig {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // Reqwest can enable a second Rustls provider, so choose ring explicitly
    // instead of letting Rustls panic while resolving an ambiguous default.
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports Rustls default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

/// A rustls client that accepts any certificate, used as a second, best-effort
/// attempt to still extract the peer's certificate when the chain does not
/// verify. The report marks such a certificate `verified: false`.
fn client_fallback_config() -> ClientConfig {
    ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports Rustls default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAllVerifier))
        .with_no_client_auth()
}

lazy_static! {
    static ref VERIFY_CONNECTOR: Arc<TlsConnector> =
        Arc::new(TlsConnector::from(Arc::new(client_verify_config())));
    static ref FALLBACK_CONNECTOR: Arc<TlsConnector> =
        Arc::new(TlsConnector::from(Arc::new(client_fallback_config())));
}

/// A certificate verifier that accepts any certificate. Only used as a second,
/// best-effort attempt to extract the peer's certificate; the report still
/// marks it `verified: false`.
#[derive(Debug)]
struct AcceptAllVerifier;

impl ServerCertVerifier for AcceptAllVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ECDSA_NISTP521_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

async fn probe_one(client: &reqwest::Client, url: &str) -> Probe {
    let start = Instant::now();
    let mut resp = match client.get(url).send().await {
        Ok(r) => r,
        Err(_) => {
            return Probe {
                result: ProbeResult {
                    url: url.to_string(),
                    status_code: 0,
                    status_text: "connection error".to_string(),
                    content_type: None,
                    body_size: 0,
                    timing_ms: start.elapsed().as_secs_f64() * 1000.0,
                    ttfb_ms: 0.0,
                    redirect_location: None,
                    headers: BTreeMap::new(),
                    sensitive_file_signature: false,
                },
                body: String::new(),
                raw_set_cookie: None,
                error: Some("No endpoint returned an HTTP response.".to_string()),
            };
        }
    };
    // TTFB is the time to the header-only response; total adds the body read.
    let ttfb = start.elapsed();
    let status = resp.status().as_u16();
    // Everything that borrows `resp` must happen before `text()` consumes it.
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let redirect_location = resp
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let status_text = resp
        .status()
        .canonical_reason()
        .unwrap_or_default()
        .to_string();
    let mut headers = BTreeMap::new();
    let mut raw_set_cookie = None;
    for (name, value) in resp.headers().iter() {
        let lname = name.to_string().to_lowercase();
        if lname == "set-cookie" {
            raw_set_cookie = Some(value.to_str().unwrap_or("").to_string());
            headers.insert(lname, sanitize_set_cookie(value));
        } else if is_reportable_header(&lname) {
            headers.insert(lname, value.to_str().unwrap_or("[binary]").to_string());
        }
    }
    let mut bytes = Vec::new();
    while bytes.len() < MAX_RESPONSE_BODY {
        let chunk = match resp.chunk().await {
            Ok(Some(chunk)) => chunk,
            Ok(None) | Err(_) => break,
        };
        let remaining = MAX_RESPONSE_BODY - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    let body = String::from_utf8_lossy(&bytes).into_owned();
    let timing = start.elapsed();

    Probe::ok(
        ProbeResult {
            url: url.to_string(),
            status_code: status,
            status_text,
            content_type,
            body_size: body.len(),
            timing_ms: timing.as_secs_f64() * 1000.0,
            ttfb_ms: ttfb.as_secs_f64() * 1000.0,
            redirect_location,
            headers,
            sensitive_file_signature: sensitive_file_signature(url, &bytes),
        },
        body,
        raw_set_cookie,
    )
}

fn is_reportable_header(name: &str) -> bool {
    matches!(
        name,
        "access-control-allow-origin"
            | "content-security-policy"
            | "referrer-policy"
            | "server"
            | "strict-transport-security"
            | "x-content-type-options"
            | "x-frame-options"
            | "x-odoo-debug"
            | "x-odoo-version"
            | "x-openerp-version"
    )
}

fn sanitize_set_cookie(value: &HeaderValue) -> String {
    let mut parts = value.to_str().unwrap_or("").split(';');
    let first = parts.next().unwrap_or(value.to_str().unwrap_or(""));
    let name = first
        .split_once('=')
        .map(|(n, _)| n.trim())
        .unwrap_or("cookie");
    if name.is_empty() {
        return "[redacted]".to_string();
    }
    let mut out = format!("{name}=[redacted]");
    for flag in parts {
        let flag = flag.trim();
        if !flag.is_empty() {
            out.push_str(&format!("; {flag}"));
        }
    }
    out
}

/// Pick the endpoint whose headers/cookies are most representative: the main
/// `/web` entry, else the first 2xx, else the first probe.
fn pick_main(probes: &[Probe]) -> Option<Probe> {
    probes
        .iter()
        .find(|p| p.result.url.ends_with("/web"))
        .or_else(|| {
            probes
                .iter()
                .find(|p| p.result.status_code >= 200 && p.result.status_code < 300)
        })
        .or_else(|| probes.first())
        .cloned()
}

/// Version comes first from the `x-openerp-version` or `x-odoo-version` header,
/// present on virtually every Odoo response), then from the web app manifest,
/// then from the public `/website/info` page's own "Odoo Version" marker.
/// Module names cannot establish a runtime version: the catalog is incomplete
/// and private ports/backports can exist.
fn detect_version(probes: &[Probe]) -> VersionInfo {
    for p in probes {
        for header in ["x-openerp-version", "x-odoo-version"] {
            if let Some(v) = p.result.headers.get(header) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    return version_detected(v, "header");
                }
            }
        }
    }
    for p in probes {
        if p.result.url.ends_with("manifest.json") && p.body.trim().starts_with('{') {
            if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&p.body) {
                if let Some(s) = map.get("version").and_then(|v| v.as_str()) {
                    if !s.is_empty() {
                        return version_detected(s.to_string(), "manifest");
                    }
                }
            }
        }
    }
    // The public /website/info page renders Odoo's own version marker; it is
    // authoritative and present whenever that page is what proved Odoo.
    if let Some(v) = detect_version_from_page(probes) {
        return version_detected(v, "page");
    }
    VersionInfo {
        detected: None,
        source: "none",
        supported: false,
        status: "unknown",
    }
}

/// Extract the Odoo version from a `/website/info` page body. Odoo renders the
/// version as a `data-odoo-vsn="18.0"` attribute and/or an "Odoo Version 18.1"
/// label - either is a direct, authoritative report of the running version.
fn detect_version_from_page(probes: &[Probe]) -> Option<String> {
    let p = probes
        .iter()
        .find(|p| p.result.url.ends_with("/website/info"))?;
    if let Some(m) = VERSION_ATTR_RE.captures(&p.body) {
        return Some(m[1].to_string());
    }
    if let Some(m) = VERSION_LABEL_RE.captures(&p.body) {
        return Some(m[1].to_string());
    }
    None
}

/// Resolves the installed module names against the collector DB to find which
/// have a known org/repo, so the UI can link each one to its info page.
/// Read-only: opens a query-only pool and only issues a SELECT.
fn infer_module_links(module_names: &[String], version: Option<&str>) -> Vec<ModuleRepositoryInfo> {
    let path = std::env::var("OGHCOLLECTOR_DB_PATH").unwrap_or_else(|_| "data/data.db".to_string());
    let pool = match sqlitedb::try_read_pool(&path, 1) {
        Some(pool) => pool,
        None => return Vec::new(),
    };
    let mut conn = match pool.get() {
        Ok(conn) => conn,
        Err(_) => return Vec::new(),
    };
    match version.and_then(|version| version_key(version).split('.').next()?.parse::<u8>().ok()) {
        Some(version) => {
            sqlitedb::models::module::get_module_repository(&mut conn, &version, module_names)
        }
        None => {
            sqlitedb::models::module::get_repository_org_by_technical_names(&mut conn, module_names)
        }
    }
}

fn version_detected(raw: String, source: &'static str) -> VersionInfo {
    // The header/manifest reports the full Odoo release (e.g. "17.0.12"); the
    // support table is keyed by major.minor only, so compare that projection.
    let supported = SUPPORTED_VERSIONS.contains(&version_key(&raw).as_str());
    let major = raw.split('.').next().and_then(|v| v.parse::<u16>().ok());
    VersionInfo {
        detected: Some(raw),
        source,
        supported,
        status: if supported {
            "supported"
        } else if major.is_some_and(|v| (1..17).contains(&v)) {
            "outdated"
        } else {
            "unknown"
        },
    }
}

/// Project a full version ("17.0.12") onto its major.minor key ("17.0").
fn version_key(raw: &str) -> String {
    raw.split('.').take(2).collect::<Vec<_>>().join(".")
}

/// Returns the parsed cookies (with security flags) seen across all probes and
/// whether debug mode appears enabled.
fn analyze_session(probes: &[Probe]) -> (Vec<CookieInfo>, bool) {
    let mut cookies = Vec::new();
    for p in probes {
        if let Some(raw) = &p.raw_set_cookie {
            if let Some(c) = parse_cookie(raw) {
                // The same session cookie is set on every response, so keep only
                // the first occurrence per name - otherwise it would be reported
                // once per probe and inflate the findings.
                if !cookies
                    .iter()
                    .any(|existing: &CookieInfo| existing.name == c.name)
                {
                    cookies.push(c);
                }
            }
        }
    }
    let debug = probes.iter().any(|p| is_debug(&p.body, &p.result.headers));
    (cookies, debug)
}

fn parse_cookie(raw: &str) -> Option<CookieInfo> {
    let parts: Vec<&str> = raw.split(';').map(|s| s.trim()).collect();
    let name = parts.first()?.split_once('=')?.0.trim();
    if name.is_empty() {
        return None;
    }
    let mut secure = false;
    let mut httponly = false;
    let mut samesite = None;
    for flag in parts.iter().skip(1) {
        let lower = flag.to_lowercase();
        if lower == "secure" {
            secure = true;
        } else if lower == "httponly" {
            httponly = true;
        } else if let Some(rest) = lower.strip_prefix("samesite=") {
            samesite = Some(rest.to_string());
        }
    }
    Some(CookieInfo {
        name: name.to_string(),
        secure,
        httponly,
        samesite,
    })
}

/// Best-effort debug detection: the `x-odoo-debug` header when truthy, or a
/// debug marker in the HTML body. Conservative on purpose - a miss just means
/// no debug finding, never a false positive.
fn is_debug(body: &str, headers: &BTreeMap<String, String>) -> bool {
    if let Some(v) = headers.get("x-odoo-debug") {
        if !v.eq_ignore_ascii_case("false") && !v.eq_ignore_ascii_case("off") {
            return true;
        }
    }
    let b = body.to_lowercase();
    b.contains("debug_panel") || b.contains("o_debug") || b.contains("web.debug")
}

fn analyze_database(probes: &[Probe]) -> DatabaseInfo {
    let info = probes
        .iter()
        .find(|p| p.result.url.ends_with("/web/database/info"));
    let manager = probes
        .iter()
        .find(|p| p.result.url.ends_with("/web/database/manager"));

    let (info_available, info_status, databases) = match info {
        Some(p) if p.result.status_code == 200 && !p.body.trim().is_empty() => {
            let databases = parse_databases(&p.body);
            (!databases.is_empty(), Some(200), databases)
        }
        Some(p) => (false, Some(p.result.status_code), Vec::new()),
        None => (false, None, Vec::new()),
    };

    let manager_available = match manager {
        Some(p) => {
            (200..300).contains(&p.result.status_code)
                && [
                    "/web/database/create",
                    "/web/database/backup",
                    "/web/database/drop",
                ]
                .iter()
                .any(|path| p.body.contains(path))
        }
        None => false,
    };

    DatabaseInfo {
        info_available,
        info_status,
        databases,
        manager_available,
    }
}

fn parse_databases(body: &str) -> Vec<String> {
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) {
        if let Some(arr) = map.get("databases").and_then(|v| v.as_array()) {
            return arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
        }
    }
    Vec::new()
}

fn analyze_modules(probes: &[Probe]) -> ModuleInfo {
    // Primary source: the public "/website/info" page lists installed apps and
    // localizations without auth. Parse the technical names from its HTML.
    if let Some(p) = probes
        .iter()
        .find(|p| p.result.url.ends_with("/website/info"))
    {
        if (200..300).contains(&p.result.status_code) && !p.body.trim().is_empty() {
            let (names, truncated) = parse_module_names_from_page(&p.body);
            if !names.is_empty() {
                let count = names.len();
                return ModuleInfo {
                    page_status: Some(p.result.status_code),
                    page_content_type: p.result.content_type.clone(),
                    module_names: names,
                    module_count: count,
                    truncated,
                    module_links: Vec::new(),
                };
            }
        }
    }

    // Fallback: the JSON /web/modules endpoint (auth-gated on many deployments).
    match probes
        .iter()
        .find(|p| p.result.url.ends_with("/web/modules"))
    {
        Some(p) if p.result.status_code == 200 && !p.body.trim().is_empty() => {
            let names = parse_module_names(&p.body);
            let count = names.len();
            ModuleInfo {
                page_status: Some(200),
                page_content_type: p.result.content_type.clone(),
                module_names: names,
                module_count: count,
                truncated: count == MAX_MODULES,
                module_links: Vec::new(),
            }
        }
        Some(p) => ModuleInfo {
            page_status: Some(p.result.status_code),
            page_content_type: p.result.content_type.clone(),
            module_names: Vec::new(),
            module_count: 0,
            truncated: false,
            module_links: Vec::new(),
        },
        None => ModuleInfo {
            page_status: None,
            page_content_type: None,
            module_names: Vec::new(),
            module_count: 0,
            truncated: false,
            module_links: Vec::new(),
        },
    }
}

/// Best-effort extraction of installed module names from `/web/modules`. The
/// endpoint's exact JSON shape varies across Odoo versions (and is gated by
/// auth on many deployments), so this only understands the common shapes and
/// returns an empty list otherwise - the report still carries the endpoint
/// status so the caller knows enumeration was attempted.
fn parse_module_names(body: &str) -> Vec<String> {
    let trimmed = body.trim();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
        return Vec::new();
    };
    let names = match value {
        // A bare JSON array of module objects.
        Value::Array(items) => items
            .iter()
            .filter_map(|item| string_field(item, NAME_KEYS))
            .collect(),
        // {"modules": [...]} or {"data": [...]} wrappers.
        Value::Object(map) => {
            let mut names = Vec::new();
            for key in ["modules", "data"] {
                if let Some(arr) = map.get(key).and_then(|v| v.as_array()) {
                    for item in arr {
                        if let Some(s) = string_field(item, NAME_KEYS) {
                            names.push(s);
                        }
                    }
                }
            }
            names
        }
        _ => Vec::new(),
    };
    limit_module_names(names.into_iter()).0
}

/// Field names that may carry a module's technical name, tried in order.
const NAME_KEYS: &[&str] = &["technical_name", "name", "module"];

fn string_field(item: &Value, keys: &[&str]) -> Option<String> {
    for k in keys {
        if let Some(s) = item.get(k).and_then(|v| v.as_str()) {
            if !s.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Concrete files whose body format can be checked; a 200 alone proves nothing.
const SENSITIVE_PATHS: &[(&str, &str, &str)] = &[
    ("/.git/HEAD", "high", "A Git HEAD signature is served at /.git/HEAD. Block access to .git and check whether repository objects or secrets were exposed."),
    ("/.env", "high", "Environment-variable assignments are served at /.env. Remove public access and rotate any exposed credentials after reviewing the file."),
    ("/.DS_Store", "low", "A .DS_Store file is served, leaking local file names."),
];

fn sensitive_file_signature(url: &str, bytes: &[u8]) -> bool {
    let body = String::from_utf8_lossy(bytes);
    let body = body.trim();
    if url.ends_with("/.git/HEAD") {
        body.strip_prefix("ref: refs/").is_some_and(|reference| {
            !reference.is_empty() && !reference.chars().any(char::is_whitespace)
        }) || (matches!(body.len(), 40 | 64) && body.bytes().all(|b| b.is_ascii_hexdigit()))
    } else if url.ends_with("/.env") {
        body.lines().any(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            line.split_once('=').is_some_and(|(key, _)| {
                let key = key.trim();
                !key.is_empty()
                    && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                    && !key.as_bytes()[0].is_ascii_digit()
            })
        })
    } else {
        url.ends_with("/.DS_Store") && bytes.starts_with(b"\x00\x00\x00\x01Bud1")
    }
}

fn analyze_sensitive_files(endpoints: &[ProbeResult]) -> (Vec<Finding>, SecurityCheck) {
    let mut findings = Vec::new();
    let mut leaked = Vec::new();
    for e in endpoints {
        for (path, severity, message) in SENSITIVE_PATHS {
            if e.url.ends_with(*path)
                && e.status_code == 200
                && e.sensitive_file_signature
                && !e
                    .content_type
                    .as_deref()
                    .is_some_and(|content_type| content_type.starts_with("text/html"))
            {
                findings.push(Finding::new(
                    severity,
                    "sensitive-file",
                    message.to_string(),
                ));
                leaked.push(format!("{path} -> {}", e.status_code));
            }
        }
    }
    if leaked.is_empty() {
        (
            findings,
            SecurityCheck {
                code: "sensitive-files".into(),
                passed: SENSITIVE_PATHS.iter().all(|(path, _, _)|
                    endpoints.iter().any(|e| e.url.ends_with(path)
                        && matches!(e.status_code, 401 | 403 | 404 | 410)))
                    .then_some(true),
                detail: "No sensitive-file signature found. Redirects, errors and unrecognized bodies are inconclusive; only the listed paths were checked.".into(),
            },
        )
    } else {
        (
            findings,
            SecurityCheck {
                code: "sensitive-files".into(),
                passed: Some(false),
                detail: leaked.join("; "),
            },
        )
    }
}

fn analyze_cors(endpoints: &[ProbeResult]) -> (Vec<Finding>, SecurityCheck) {
    let Some(main) = endpoints
        .iter()
        .find(|e| e.url.ends_with("/web") && (200..300).contains(&e.status_code))
    else {
        return (
            Vec::new(),
            SecurityCheck {
                code: "cors".into(),
                passed: None,
                detail: "No response headers to evaluate.".into(),
            },
        );
    };
    let Some(cao) = main
        .headers
        .get("access-control-allow-origin")
        .map(|v| v.as_str())
    else {
        return (
            Vec::new(),
            SecurityCheck {
                code: "cors".into(),
                passed: Some(true),
                detail: "No Access-Control-Allow-Origin header.".into(),
            },
        );
    };
    if cao == "*" {
        (
            vec![Finding::new(
                "info",
                "permissive-cors",
                "Access-Control-Allow-Origin: * on /web permits cross-origin reads without credentials. Browsers reject wildcard origins for credentialed reads; verify whether this public response is intended to be shared.",
            )],
            SecurityCheck {
                code: "cors".into(),
                passed: None,
                detail: "Access-Control-Allow-Origin: *".into(),
            },
        )
    } else {
        (
            Vec::new(),
            SecurityCheck {
                code: "cors".into(),
                passed: Some(true),
                detail: format!("Access-Control-Allow-Origin: {cao}"),
            },
        )
    }
}

fn analyze_certificate(
    certificate: Option<&CertificateInfo>,
    is_https: bool,
) -> (Vec<Finding>, SecurityCheck) {
    if !is_https {
        return (
            Vec::new(),
            SecurityCheck {
                code: "certificate".into(),
                passed: Some(true),
                detail: "Not applicable: the target uses HTTP.".into(),
            },
        );
    }
    match certificate {
        Some(cert) if cert.verified && cert.valid_now => (
            Vec::new(),
            SecurityCheck {
                code: "certificate".into(),
                passed: Some(true),
                detail: "TLS certificate is trusted and currently valid.".into(),
            },
        ),
        Some(cert) => {
            let reason = if !cert.valid_now {
                "expired or not yet valid"
            } else {
                "untrusted or does not match the hostname"
            };
            (
                vec![Finding::new(
                    "high",
                    "invalid-certificate",
                    format!("TLS certificate is {reason}."),
                )],
                SecurityCheck {
                    code: "certificate".into(),
                    passed: Some(false),
                    detail: format!("TLS certificate is {reason}."),
                },
            )
        }
        None => (
            Vec::new(),
            SecurityCheck {
                code: "certificate".into(),
                passed: None,
                detail: "TLS certificate could not be inspected.".into(),
            },
        ),
    }
}

/// Run the security-surface checks and collect their pass/fail results. The
/// detailed findings come from `build_findings`; this is the checklist view.
fn collect_checks(report: &ScanReport) -> Vec<SecurityCheck> {
    let mut checks = Vec::new();
    let (_, sensitive) = analyze_sensitive_files(&report.endpoints);
    checks.push(sensitive);
    let (_, cors) = analyze_cors(&report.endpoints);
    checks.push(cors);
    let (_, certificate) = analyze_certificate(report.certificate.as_ref(), report.is_https);
    checks.push(certificate);
    checks
}

/// Build review findings from the observed signals.
fn build_findings(report: &ScanReport) -> Vec<Finding> {
    if !report.reachable {
        return report
            .error
            .as_ref()
            .map(|e| vec![Finding::new("info", "unreachable", e.clone())])
            .unwrap_or_else(|| {
                vec![Finding::new(
                    "info",
                    "unreachable",
                    "No probe received an HTTP response.",
                )]
            });
    }

    let mut findings = Vec::new();

    // Version assessment is Odoo-specific, but endpoint exposure and HTTP
    // hardening findings remain valid when a proxy hides Odoo's fingerprints.
    if !report.is_odoo {
        findings.push(Finding::new(
            "info",
            "not-odoo",
            "The target responded but does not look like an Odoo web application; \
             version checks and module enumeration were skipped.",
        ));
    } else {
        match report.version.status {
            "unknown" => findings.push(Finding::new(
                "info",
                "version-unknown",
                "Could not determine the Odoo version from the response headers or the web \
                 manifest. Version-specific checks below are therefore limited.",
            )),
            "outdated" => findings.push(Finding::new(
                "high",
                "version-outdated",
                format!(
                    "Odoo {} is outside the standard support versions {:?} (matrix: September 2026). \
                      Verify vendor/extended support and deployed security patches; plan an upgrade if unsupported.",
                    report.version.detected.clone().unwrap_or_default(),
                    SUPPORTED_VERSIONS
                ),
            )),
            _ => {}
        }
    }

    if report.database.info_available {
        findings.push(Finding::new(
            "medium",
            "db-info-exposed",
            "The /web/database/info response lists database names. Disable database listing \
             (list_db = False) or restrict these endpoints. This does not demonstrate permission to delete databases.",
        ));
    }

    if report.database.manager_available {
        findings.push(Finding::new(
            "medium",
            "db-manager-exposed",
            "Database-manager forms are publicly visible at /web/database/manager. Restrict access \
             and protect the master password (admin_passwd). Management operations were not attempted.",
        ));
    }

    if report.debug_mode {
        findings.push(Finding::new(
            "info",
            "debug-mode",
            "Odoo developer-mode markers were observed. Developer mode is not an authentication \
             bypass or evidence of an exposed server-side debugger.",
        ));
    }

    if let Some(main) = &report.main_endpoint {
        if (200..300).contains(&main.status_code) {
            let headers = &main.headers;
            if report.is_https && headers.get("strict-transport-security").is_none() {
                findings.push(Finding::new(
                    "medium",
                    "missing-hsts",
                    "No Strict-Transport-Security header: browsers will not be forced onto HTTPS.",
                ));
            }
            if headers.get("content-security-policy").is_none() {
                findings.push(Finding::new(
                    "low",
                    "missing-csp",
                    "No Content-Security-Policy header set on the response.",
                ));
            }
            let frame_ancestors = headers.get("content-security-policy").is_some_and(|csp| {
                csp.split(';')
                    .any(|directive| directive.split_whitespace().next() == Some("frame-ancestors"))
            });
            if headers.get("x-frame-options").is_none() && !frame_ancestors {
                findings.push(Finding::new(
                "medium",
                "missing-xfo",
                "No X-Frame-Options or CSP frame-ancestors directive on this response. Review framing restrictions for sensitive pages.",
            ));
            }
            if headers.get("x-content-type-options").is_none() {
                findings.push(Finding::new(
                    "low",
                    "missing-xcto",
                    "No X-Content-Type-Options header: MIME-sniffing is not discouraged.",
                ));
            }
            if headers.get("referrer-policy").is_none() {
                findings.push(Finding::new(
                    "low",
                    "missing-referrer-policy",
                    "No Referrer-Policy header: referrer info may leak outside the site.",
                ));
            }
            if let Some(server) = &report.server_header {
                if server_leaks_version(server) {
                    findings.push(Finding::new(
                        "info",
                        "server-leak",
                        format!(
                            "The Server header '{server}' reveals a software version, aiding \
                         attackers in targeting known vulnerabilities.",
                        ),
                    ));
                }
            }
        }
    }

    for cookie in &report.cookies {
        if !cookie.httponly {
            findings.push(Finding::new(
                "high",
                format!("cookie-{}-no-httponly", cookie.name),
                format!(
                    "Session cookie '{}' is not HttpOnly: JavaScript can read it, so an XSS \
                     payload can steal the session.",
                    cookie.name
                ),
            ));
        }
        if !cookie.secure {
            findings.push(Finding::new(
                "medium",
                format!("cookie-{}-no-secure", cookie.name),
                format!(
                    "Session cookie '{}' is not Secure: it can be sent over plaintext HTTP.",
                    cookie.name
                ),
            ));
        }
        if cookie.samesite.is_none() {
            findings.push(Finding::new(
                "low",
                format!("cookie-{}-no-samesite", cookie.name),
                format!(
                    "Session cookie '{}' omits SameSite. Modern browsers usually default to Lax; set an explicit policy appropriate to login/payment flows. This alone does not prove CSRF.",
                    cookie.name
                ),
            ));
        }
    }

    if !report.is_https {
        findings.push(Finding::new(
            "high",
            "no-https",
            "The instance is served over plain HTTP, not HTTPS. Credentials and session \
             cookies travel in the clear.",
        ));
    }

    let (sensitive, _) = analyze_sensitive_files(&report.endpoints);
    findings.extend(sensitive);
    let (cors, _) = analyze_cors(&report.endpoints);
    findings.extend(cors);
    let (certificate, _) = analyze_certificate(report.certificate.as_ref(), report.is_https);
    findings.extend(certificate);

    findings
}

/// Heuristic: does the Server header contain a dotted version number like
/// "nginx/1.24.0"? Avoids pulling in regex for a one-shot check.
fn server_leaks_version(server: &str) -> bool {
    let bytes = server.as_bytes();
    bytes
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == b'.' && w[2].is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(url: &str, status: u16, body: &str) -> Probe {
        let mut headers = BTreeMap::new();
        headers.insert("content-type".to_string(), "text/html".to_string());
        Probe::ok(
            ProbeResult {
                url: url.to_string(),
                status_code: status,
                status_text: status.to_string(),
                content_type: Some("text/html".to_string()),
                body_size: body.len(),
                timing_ms: 10.0,
                ttfb_ms: 5.0,
                redirect_location: None,
                headers,
                sensitive_file_signature: sensitive_file_signature(url, body.as_bytes()),
            },
            body.to_string(),
            None,
        )
    }

    #[test]
    fn normalize_url_defaults_to_https_and_strips_trailing_slash() {
        assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
        assert_eq!(
            normalize_url("https://example.com/").unwrap(),
            "https://example.com"
        );
        assert_eq!(
            normalize_url("http://example.com/").unwrap(),
            "http://example.com"
        );
        assert!(normalize_url("").is_err());
        assert!(normalize_url("   ").is_err());
        assert!(normalize_url("not a url").is_err());
    }

    #[tokio::test]
    async fn validate_target_rejects_non_public_addresses() {
        // IP literals are checked directly, no DNS needed.
        for blocked in [
            "http://127.0.0.1/",
            "http://0.0.0.0/",
            "http://10.1.2.3/",
            "http://100.64.0.1/",
            "http://198.18.0.1/",
            "http://192.168.0.1/",
            "http://169.254.169.254/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(validate_target(blocked).await.is_err(), "{blocked}");
        }
    }

    #[tokio::test]
    async fn validate_target_rejects_non_http_scheme() {
        assert!(validate_target("ftp://example.com").await.is_err());
        assert!(validate_target("file:///etc/passwd").await.is_err());
        assert!(validate_target("javascript:alert(1)").await.is_err());
    }

    #[test]
    fn normalize_url_rejects_paths_and_credentials() {
        assert!(normalize_url("https://example.com/web").is_err());
        assert!(normalize_url("https://user:password@example.com").is_err());
    }

    #[test]
    fn version_detected_marks_support_status() {
        let supported = version_detected("18.0".to_string(), "header");
        assert_eq!(supported.status, "supported");
        assert!(supported.supported);

        let outdated = version_detected("14.0".to_string(), "header");
        assert_eq!(outdated.status, "outdated");
        assert!(!outdated.supported);
    }

    #[test]
    fn detect_version_prefers_header_over_manifest() {
        let mut probes = vec![
            probe("https://x/web", 200, ""),
            probe(
                "https://x/web/static/manifest.json",
                200,
                "{\"version\":\"18.0.5\"}",
            ),
        ];
        // Give /web the version header; the manifest also carries a (different)
        // version - the header must win.
        probes[0]
            .result
            .headers
            .insert("x-openerp-version".to_string(), "17.0.3".to_string());
        let v = detect_version(&probes);
        assert_eq!(v.detected.as_deref(), Some("17.0.3"));
        assert_eq!(v.source, "header");
        assert_eq!(v.status, "supported");
    }

    #[test]
    fn detect_version_falls_back_to_manifest_header_absent() {
        let probes = vec![probe(
            "https://x/web/static/manifest.json",
            200,
            "{\"version\":\"16.1\"}",
        )];
        let v = detect_version(&probes);
        assert_eq!(v.detected.as_deref(), Some("16.1"));
        assert_eq!(v.source, "manifest");
    }

    #[test]
    fn detect_version_none_when_nothing_present() {
        let probes = vec![probe("https://x/web", 200, "<html>nothing</html>")];
        let v = detect_version(&probes);
        assert_eq!(v.status, "unknown");
        assert_eq!(v.source, "none");
    }

    #[test]
    fn detect_version_reads_vsn_attribute_from_info_page() {
        let probes = vec![probe(
            "https://x/website/info",
            200,
            "<html data-odoo-vsn=\"18.0\">",
        )];
        let v = detect_version(&probes);
        assert_eq!(v.detected.as_deref(), Some("18.0"));
        assert_eq!(v.source, "page");
    }

    #[test]
    fn detect_version_reads_version_label_from_info_page() {
        let probes = vec![probe("https://x/website/info", 200, "Odoo Version 17.4")];
        let v = detect_version(&probes);
        assert_eq!(v.detected.as_deref(), Some("17.4"));
        assert_eq!(v.source, "page");
    }

    #[test]
    fn parse_cookie_extracts_security_flags() {
        let c = parse_cookie("session_id=abc123; HttpOnly; Secure; SameSite=Strict").unwrap();
        assert_eq!(c.name, "session_id");
        assert!(c.httponly);
        assert!(c.secure);
        assert_eq!(c.samesite.as_deref(), Some("strict"));
    }

    #[test]
    fn parse_cookie_handles_missing_flags() {
        let c = parse_cookie("oidc_session=xyz").unwrap();
        assert_eq!(c.name, "oidc_session");
        assert!(!c.httponly);
        assert!(!c.secure);
        assert!(c.samesite.is_none());
    }

    #[test]
    fn parse_cookie_rejects_empty_name() {
        assert!(parse_cookie("; HttpOnly").is_none());
    }

    #[test]
    fn analyze_database_detects_exposed_enumeration() {
        let probes = vec![
            probe(
                "https://x/web/database/info",
                200,
                "{\"databases\":[\"prod\",\"test\"]}",
            ),
            probe(
                "https://x/web/database/manager",
                200,
                "<form action='/web/database/backup'></form>",
            ),
        ];
        let db = analyze_database(&probes);
        assert!(db.info_available);
        assert_eq!(db.databases, vec!["prod".to_string(), "test".to_string()]);
        assert!(db.manager_available);
    }

    #[test]
    fn analyze_database_reports_hidden_info_endpoint() {
        let probes = vec![probe("https://x/web/database/info", 404, "")];
        let db = analyze_database(&probes);
        assert!(!db.info_available);
        assert_eq!(db.info_status, Some(404));
    }

    #[test]
    fn parse_module_names_understands_common_shapes() {
        let json =
            r#"{"modules":[{"technical_name":"sale","name":"Sale"},{"technical_name":"crm"}]}"#;
        let names = parse_module_names(json);
        assert_eq!(names, vec!["sale".to_string(), "crm".to_string()]);

        // A bare array: any of the recognized name keys is accepted.
        let arr = r#"[{"name":"stock"},{"module":"website"}]"#;
        assert_eq!(
            parse_module_names(arr),
            vec!["stock".to_string(), "website".to_string()]
        );

        // Not JSON - empty, never panics.
        assert!(parse_module_names("<html>apps</html>").is_empty());
    }

    #[test]
    fn is_odoo_detects_older_and_modern_version_headers() {
        // Odoo <= 16 sends x-openerp-version.
        let mut legacy = probe("https://x/web", 200, "");
        legacy
            .result
            .headers
            .insert("x-openerp-version".to_string(), "16.0".to_string());
        assert!(is_odoo(&[legacy]));

        // Odoo 17+ renamed the header to x-odoo-version.
        let mut modern = probe("https://x/web", 200, "");
        modern
            .result
            .headers
            .insert("x-odoo-version".to_string(), "18.1".to_string());
        assert!(is_odoo(&[modern]));

        let mut modern = probe("https://x/web", 200, "");
        modern
            .result
            .headers
            .insert("x-odoo-version".to_string(), "18.1".to_string());
        assert_eq!(detect_version(&[modern]).detected.as_deref(), Some("18.1"));
    }

    #[test]
    fn is_odoo_detects_manifest_and_page_markers_without_header() {
        let manifest = probe(
            "https://x/web/static/manifest.json",
            200,
            r#"{"modules":{"sale":{}}}"#,
        );
        assert!(is_odoo(&[manifest]));

        let info = probe(
            "https://x/website/info",
            200,
            "<h1>Installed Applications</h1><p>Odoo Version 18.1</p>",
        );
        assert!(is_odoo(&[info]));
    }

    #[test]
    fn is_odoo_detects_assets_frontend_when_header_stripped() {
        // A real page whose version header was stripped by a proxy still
        // references the Odoo frontend bundle in its <head>.
        let body = "<html><head><link rel=\"stylesheet\" href=\"/web/assets/web.assets_frontend.min.css\"></head><body></body></html>";
        let page = probe("https://x/web", 200, body);
        assert!(is_odoo(&[page]));

        // A non-Odoo page without any Odoo signal is not detected.
        let other = probe(
            "https://x/home",
            200,
            "<html><head><title>Shop</title></head></html>",
        );
        assert!(!is_odoo(&[other]));
    }

    #[test]
    fn is_debug_triggers_on_header_and_body_markers() {
        let mut headers = BTreeMap::new();
        headers.insert("x-odoo-debug".to_string(), "True".to_string());
        assert!(is_debug("<html></html>", &headers));

        let empty = BTreeMap::new();
        assert!(!is_debug("<html>a perfectly ordinary page</html>", &empty));
        assert!(!is_debug("<html>plain page</html>", &empty));
    }

    #[test]
    fn build_findings_reports_evidence_without_claiming_database_deletion() {
        let mut main = probe("https://x/web", 200, "");
        main.result
            .headers
            .insert("server".to_string(), "nginx/1.24.0".to_string());
        let report = ScanReport {
            target_url: "https://x".to_string(),
            reachable: true,
            error: None,
            version: version_detected("14.0".to_string(), "header"),
            is_https: true,
            debug_mode: true,
            timings: Timings {
                total_ms: 70.0,
                main_ttfb_ms: 5.0,
                main_total_ms: 4000.0,
            },
            database: DatabaseInfo {
                info_available: true,
                info_status: Some(200),
                databases: vec!["prod".to_string()],
                manager_available: true,
            },
            modules: ModuleInfo {
                page_status: Some(200),
                page_content_type: None,
                module_names: Vec::new(),
                module_count: 0,
                truncated: false,
                module_links: Vec::new(),
            },
            cookies: vec![CookieInfo {
                name: "session".to_string(),
                secure: false,
                httponly: false,
                samesite: None,
            }],
            endpoints: vec![main.result.clone()],
            main_endpoint: Some(main.result),
            server_header: Some("nginx/1.24.0".to_string()),
            is_odoo: true,
            certificate: None,
            findings: Vec::new(),
            checks: Vec::new(),
        };
        let findings = build_findings(&report);
        let codes: Vec<&str> = findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"db-info-exposed"));
        assert!(codes.contains(&"db-manager-exposed"));
        assert!(codes.contains(&"debug-mode"));
        assert!(codes.contains(&"version-outdated"));
        assert!(codes.contains(&"cookie-session-no-httponly"));
        assert!(codes.contains(&"cookie-session-no-secure"));
        assert!(codes.contains(&"cookie-session-no-samesite"));
        assert!(codes.contains(&"missing-hsts"));
        assert!(codes.contains(&"server-leak"));
        assert!(!codes.contains(&"slow-response"));
        assert!(!findings.iter().any(|f| f.severity == "critical"));
        assert_eq!(
            findings
                .iter()
                .find(|f| f.code == "debug-mode")
                .unwrap()
                .severity,
            "info"
        );

        let mut unclassified = report;
        unclassified.is_odoo = false;
        let findings = build_findings(&unclassified);
        assert!(findings.iter().any(|f| f.code == "db-info-exposed"));
    }

    #[test]
    fn build_findings_clean_instance_has_no_findings() {
        let mut main = probe("https://x/web", 200, "");
        main.result.headers.extend([
            (
                "strict-transport-security".to_string(),
                "max-age=31536000".to_string(),
            ),
            (
                "content-security-policy".to_string(),
                "default-src 'self'".to_string(),
            ),
            ("x-frame-options".to_string(), "DENY".to_string()),
            ("x-content-type-options".to_string(), "nosniff".to_string()),
            ("referrer-policy".to_string(), "no-referrer".to_string()),
        ]);
        let report = ScanReport {
            target_url: "https://x".to_string(),
            reachable: true,
            error: None,
            version: version_detected("18.0".to_string(), "header"),
            is_https: true,
            debug_mode: false,
            timings: Timings {
                total_ms: 70.0,
                main_ttfb_ms: 5.0,
                main_total_ms: 200.0,
            },
            database: DatabaseInfo {
                info_available: false,
                info_status: Some(404),
                databases: Vec::new(),
                manager_available: false,
            },
            modules: ModuleInfo {
                page_status: Some(404),
                page_content_type: None,
                module_names: Vec::new(),
                module_count: 0,
                truncated: false,
                module_links: Vec::new(),
            },
            cookies: vec![CookieInfo {
                name: "session".to_string(),
                secure: true,
                httponly: true,
                samesite: Some("Strict".to_string()),
            }],
            endpoints: vec![main.result.clone()],
            main_endpoint: Some(main.result),
            server_header: Some("nginx".to_string()),
            is_odoo: true,
            certificate: None,
            findings: Vec::new(),
            checks: Vec::new(),
        };
        assert!(build_findings(&report).is_empty());
        let mut report = report;
        let main = report.main_endpoint.as_mut().unwrap();
        main.headers.remove("x-frame-options");
        main.headers.insert(
            "content-security-policy".into(),
            "frame-ancestors 'self'".into(),
        );
        assert!(!build_findings(&report)
            .iter()
            .any(|f| f.code == "missing-xfo"));
        report.main_endpoint.as_mut().unwrap().status_code = 302;
        report.main_endpoint.as_mut().unwrap().headers.clear();
        assert!(!build_findings(&report)
            .iter()
            .any(|f| f.code.starts_with("missing-")));
    }

    #[test]
    fn sanitize_set_cookie_keeps_flags_redacts_value() {
        let v =
            HeaderValue::from_static("sessionid=supersecret; HttpOnly; Secure; SameSite=Strict");
        assert_eq!(
            sanitize_set_cookie(&v),
            "sessionid=[redacted]; HttpOnly; Secure; SameSite=Strict"
        );
    }

    #[test]
    fn server_leaks_version_detects_dotted_numbers_only() {
        assert!(server_leaks_version("nginx/1.24.0"));
        assert!(!server_leaks_version("nginx"));
        assert!(!server_leaks_version("Apache"));
    }

    #[test]
    fn scan_report_fatal_shapes_a_complete_report() {
        let report = ScanReport::fatal("example.com", "invalid URL 'x'");
        assert!(!report.reachable);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].code, "unreachable");
        assert_eq!(report.findings[0].severity, "info");
        assert_eq!(report.checks[0].passed, None);
        assert_eq!(report.checks.len(), 1);
    }

    #[test]
    fn sensitive_file_checks_require_body_evidence_and_do_not_pass_missing_probes() {
        assert!(sensitive_file_signature(
            "https://x/.git/HEAD",
            b"ref: refs/heads/main\n"
        ));
        assert!(sensitive_file_signature(
            "https://x/.env",
            b"export DB_PASSWORD=example\n"
        ));
        assert!(sensitive_file_signature(
            "https://x/.DS_Store",
            b"\x00\x00\x00\x01Bud1"
        ));
        for body in [b"Not found".as_slice(), b"<html>Welcome</html>", b""] {
            assert!(!sensitive_file_signature("https://x/.git/HEAD", body));
            assert!(!sensitive_file_signature("https://x/.env", body));
        }
        let fallback = probe("https://x/.env", 200, "Not found").result;
        let (findings, check) = analyze_sensitive_files(&[fallback]);
        assert!(findings.is_empty());
        assert_eq!(check.passed, None);
        assert_eq!(analyze_sensitive_files(&[]).1.passed, None);
        assert_eq!(analyze_cors(&[]).1.passed, None);
        let denied = SENSITIVE_PATHS
            .iter()
            .map(|(path, _, _)| probe(&format!("https://x{path}"), 404, "Not found").result)
            .collect::<Vec<_>>();
        assert_eq!(analyze_sensitive_files(&denied).1.passed, Some(true));
    }

    #[test]
    fn analyze_sensitive_files_flags_200_paths_only() {
        let endpoints = vec![
            ProbeResult {
                url: "https://x/.git/HEAD".into(),
                status_code: 200,
                sensitive_file_signature: true,
                ..Default::default()
            },
            ProbeResult {
                url: "https://x/.env".into(),
                status_code: 200,
                sensitive_file_signature: true,
                ..Default::default()
            },
            ProbeResult {
                url: "https://x/robots.txt".into(),
                status_code: 200,
                ..Default::default()
            },
            ProbeResult {
                url: "https://x/.DS_Store".into(),
                status_code: 404,
                ..Default::default()
            },
        ];
        let (findings, check) = analyze_sensitive_files(&endpoints);
        assert_eq!(check.passed, Some(false));
        // Only the two 200 paths leak; the 404 and robots.txt do not.
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.code == "sensitive-file"));
    }

    #[test]
    fn analyze_sensitive_files_clean_when_nothing_leaks() {
        let endpoints = vec![ProbeResult {
            url: "https://x/.git/HEAD".into(),
            status_code: 404,
            ..Default::default()
        }];
        let (findings, check) = analyze_sensitive_files(&endpoints);
        assert_eq!(check.passed, None); // Other paths were not probed.
        assert!(findings.is_empty());
    }

    #[test]
    fn analyze_sensitive_files_ignores_html_fallbacks() {
        let endpoints = vec![ProbeResult {
            url: "https://x/.env".into(),
            status_code: 200,
            content_type: Some("text/html; charset=utf-8".into()),
            ..Default::default()
        }];
        let (findings, check) = analyze_sensitive_files(&endpoints);
        assert_eq!(check.passed, None); // A fallback is not a confirmed denial.
        assert!(findings.is_empty());
    }

    #[test]
    fn analyze_cors_flags_wildcard_only() {
        let headers = {
            let mut h = BTreeMap::new();
            h.insert("access-control-allow-origin".to_string(), "*".to_string());
            h
        };
        let endpoints = vec![ProbeResult {
            url: "https://x/web".into(),
            status_code: 200,
            headers,
            ..Default::default()
        }];
        let (findings, check) = analyze_cors(&endpoints);
        assert_eq!(check.passed, None);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, "permissive-cors");
        assert_eq!(findings[0].severity, "info");
    }

    #[test]
    fn analyze_cors_passes_without_header() {
        let endpoints = vec![ProbeResult {
            url: "https://x/web".into(),
            status_code: 200,
            ..Default::default()
        }];
        let (_, check) = analyze_cors(&endpoints);
        assert_eq!(check.passed, Some(true));
    }

    #[test]
    fn analyze_certificate_reports_untrusted_certificates() {
        let certificate = CertificateInfo {
            subject_cn: Some("example.com".into()),
            subject_org: None,
            issuer_cn: Some("example.com".into()),
            issuer_org: None,
            not_before: Some("2026-01-01".into()),
            not_after: Some("2027-01-01".into()),
            self_signed: true,
            verified: false,
            valid_now: true,
            sans: vec!["example.com".into()],
        };
        let (findings, check) = analyze_certificate(Some(&certificate), true);
        assert_eq!(check.passed, Some(false));
        assert_eq!(findings[0].code, "invalid-certificate");
    }

    #[test]
    fn tls_clients_select_a_crypto_provider() {
        let _ = client_verify_config();
        let _ = client_fallback_config();
    }

    #[test]
    fn inconclusive_responses_are_not_database_exposure_or_obsolete_versions() {
        let db = analyze_database(&[
            probe("https://x/web/database/info", 200, "<html>Login</html>"),
            probe("https://x/web/database/manager", 200, "<html>Login</html>"),
        ]);
        assert!(!db.info_available);
        assert!(!db.manager_available);
        for raw in ["20.0", "garbage", "18.4"] {
            assert_eq!(version_detected(raw.into(), "header").status, "unknown");
        }
        assert_eq!(
            version_detected("19.0".into(), "header").status,
            "supported"
        );
    }
}
