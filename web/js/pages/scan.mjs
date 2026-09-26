// Copyright 2026 Alexandre D. Díaz
import '@scss/pages/scan.scss';

// ScanReport shape (see oghscan::ScanReport). Kept as a structural shape so a
// future field addition never silently breaks a render - we read what we show
// and ignore the rest.
const SEVERITY_CLASS = {
  critical: 'scan-sev-critical',
  high: 'scan-sev-high',
  medium: 'scan-sev-medium',
  low: 'scan-sev-low',
  info: 'scan-sev-info',
};

function createEl(tag, opts = {}) {
  const el = document.createElement(tag);
  if (opts.className) el.className = opts.className;
  if (opts.text) el.textContent = opts.text;
  for (const [key, value] of Object.entries(opts.dataset || {})) {
    el.dataset[key] = value;
  }
  return el;
}

function show(el) {
  el.classList.remove('d-none');
}
function hide(el) {
  el.classList.add('d-none');
}

function statusLabel(status) {
  switch (status) {
    case 'supported':
      return 'Supported';
    case 'outdated':
      return 'Outdated';
    default:
      return 'Unknown';
  }
}

function renderSummary(report) {
  const versionEl = document.getElementById('scan_version');
  const versionNote = document.getElementById('scan_version_note');
  const detected = report.version.detected;
  versionEl.textContent = detected || 'Unknown';
  versionNote.textContent = detected
    ? statusLabel(report.version.status)
    : `source: ${report.version.source}`;

  document.getElementById('scan_https').textContent = report.is_https
    ? 'Yes'
    : 'No';
  document.getElementById('scan_debug').textContent = report.debug_mode
    ? 'Yes'
    : 'No';

  const findings = report.findings || [];
  document.getElementById('scan_findings_count').textContent = String(
    findings.length,
  );
  const critical = findings.filter(f => f.severity === 'critical').length;
  const high = findings.filter(f => f.severity === 'high').length;
  const findingsNote = document.getElementById('scan_findings_note');
  if (critical > 0) {
    findingsNote.textContent = `${critical} critical`;
  } else if (high > 0) {
    findingsNote.textContent = `${high} high severity`;
  } else {
    findingsNote.textContent = '';
  }
}

function renderFindings(report) {
  const container = document.getElementById('scan_findings');
  container.textContent = '';
  const findings = report.findings || [];
  if (findings.length === 0) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'No critical points detected.',
      }),
    );
    return;
  }
  for (const finding of findings) {
    const row = createEl('div', {
      className: 'scan-finding ' + (SEVERITY_CLASS[finding.severity] || ''),
    });
    row.appendChild(
      createEl('span', {
        className: 'scan-finding-badge',
        text: finding.severity,
      }),
    );
    row.appendChild(
      createEl('span', {className: 'scan-finding-msg', text: finding.message}),
    );
    container.appendChild(row);
  }
}

function renderReviewSummary(report) {
  const card = document.getElementById('scan_score_card');
  const valueEl = document.getElementById('scan_score_value');
  const ratingEl = document.getElementById('scan_score_rating');
  const breakdownEl = document.getElementById('scan_score_breakdown');

  const findings = report.findings || [];
  const breakdown = Object.keys(SEVERITY_CLASS)
    .map(severity => [
      severity,
      findings.filter(f => f.severity === severity).length,
    ])
    .filter(([, count]) => count > 0);
  const severity = breakdown[0]?.[0];
  valueEl.textContent = report.reachable ? severity || 'None' : 'Unknown';
  ratingEl.textContent = 'Highest observed severity';

  card.classList.remove(
    'scan-sev-critical',
    'scan-sev-high',
    'scan-sev-medium',
    'scan-sev-low',
    'scan-sev-info',
    'scan-sev-success',
  );
  card.classList.add(SEVERITY_CLASS[severity] || 'scan-sev-info');
  breakdownEl.textContent = breakdown
    .map(([sev, count]) => `${count} ${sev}`)
    .join(', ');
}

function renderChecks(report) {
  const container = document.getElementById('scan_checks');
  container.textContent = '';
  const checks = report.checks || [];
  if (checks.length === 0) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'No security checks ran.',
      }),
    );
    return;
  }
  for (const check of checks) {
    const row = createEl('div', {className: 'scan-check-row'});
    const state =
      check.passed === true
        ? 'pass'
        : check.passed === false
          ? 'fail'
          : 'unknown';
    const badge = createEl('span', {
      className:
        'scan-check-status ' +
        (state === 'pass'
          ? 'bg-success-subtle text-success'
          : state === 'fail'
            ? 'bg-danger-subtle text-danger'
            : 'bg-secondary-subtle text-secondary'),
    });
    badge.textContent = state;
    row.appendChild(badge);
    row.appendChild(
      createEl('span', {className: 'text-body', text: check.detail}),
    );
    container.appendChild(row);
  }
}

function renderTimings(report) {
  const container = document.getElementById('scan_timings');
  container.textContent = '';
  const t = report.timings || {};
  const rows = [
    ['Total scan time', `${t.total_ms.toFixed(0)} ms`],
    ['Main endpoint TTFB', `${t.main_ttfb_ms.toFixed(0)} ms`],
    ['Main endpoint total', `${t.main_total_ms.toFixed(0)} ms`],
  ];
  for (const [label, value] of rows) {
    const row = createEl('div', {
      className: 'd-flex justify-content-between gap-3',
      dataset: {label},
    });
    row.appendChild(
      createEl('span', {className: 'text-secondary', text: label}),
    );
    row.appendChild(
      createEl('span', {className: 'scan-detail-value', text: value}),
    );
    container.appendChild(row);
  }
}

function renderDatabase(report) {
  const container = document.getElementById('scan_database');
  container.textContent = '';
  const db = report.database || {};
  if (!report.reachable) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'Instance not reachable.',
      }),
    );
    return;
  }
  const parts = [];
  if (db.info_available) {
    parts.push('info endpoint exposed');
  }
  if (db.manager_available) {
    parts.push('manager exposed');
  }
  if (parts.length === 0) {
    parts.push('no database exposure detected');
  }
  container.appendChild(
    createEl('p', {className: 'mb-2', text: parts.join(', ')}),
  );
  if (db.databases && db.databases.length > 0) {
    const list = createEl('ul', {className: 'scan-db-list'});
    for (const db_name of db.databases) {
      list.appendChild(createEl('li', {text: db_name}));
    }
    container.appendChild(list);
  }
}

function renderCertificate(report) {
  const container = document.getElementById('scan_certificate');
  container.textContent = '';
  if (!report.is_https) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'Not applicable: the target uses HTTP.',
      }),
    );
    return;
  }
  const cert = report.certificate;
  if (!cert) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'Certificate could not be inspected.',
      }),
    );
    return;
  }
  const trust =
    cert.verified && cert.valid_now
      ? 'trusted and valid'
      : !cert.valid_now
        ? 'expired or not yet valid'
        : cert.self_signed
          ? 'self-signed'
          : 'untrusted or hostname mismatch';
  const rows = [
    ['Status', trust],
    ['Subject', cert.subject_cn || cert.subject_org || 'not provided'],
    ['Issuer', cert.issuer_cn || cert.issuer_org || 'not provided'],
    [
      'Validity',
      `${cert.not_before || 'unknown'} to ${cert.not_after || 'unknown'}`,
    ],
  ];
  for (const [label, value] of rows) {
    const row = createEl('div', {
      className: 'd-flex justify-content-between gap-3',
    });
    row.appendChild(
      createEl('span', {className: 'text-secondary', text: label}),
    );
    row.appendChild(
      createEl('span', {className: 'text-break text-end', text: value}),
    );
    container.appendChild(row);
  }
  if (cert.sans && cert.sans.length > 0) {
    container.appendChild(
      createEl('div', {
        className: 'text-body-secondary text-break',
        text: `SAN: ${cert.sans.join(', ')}`,
      }),
    );
  }
}

function renderModules(report) {
  const container = document.getElementById('scan_modules');
  container.textContent = '';
  const modules = report.modules || {};
  if (!report.reachable) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'Instance not reachable.',
      }),
    );
    return;
  }
  const total = modules.module_count || modules.module_names.length;
  if (modules.page_status !== null && modules.page_status !== undefined) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-2',
        text: `Enumeration endpoint: HTTP ${modules.page_status}`,
      }),
    );
  }
  if (total === 0) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'Module enumeration not available (endpoint gated or empty).',
      }),
    );
    return;
  }
  const list = createEl('div', {className: 'd-flex flex-wrap gap-2'});
  const linksByTech = new Map(
    (modules.module_links || []).map(l => [l.technical_name, l]),
  );
  for (const name of modules.module_names) {
    const link = linksByTech.get(name);
    if (link) {
      const badge = createEl('a', {
        className:
          'badge bg-secondary rounded-pill font-monospace text-secondary text-decoration-none',
        text: name,
      });
      badge.href = `/module/${encodeURIComponent(link.organization)}/${encodeURIComponent(link.technical_name)}`;
      badge.target = '_blank';
      badge.rel = 'noopener noreferrer';
      list.appendChild(badge);
    } else {
      list.appendChild(
        createEl('span', {
          className:
            'badge bg-secondary rounded-pill font-monospace text-secondary',
          text: name,
        }),
      );
    }
  }
  container.appendChild(list);
  if (modules.truncated) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: `Showing the first ${total} modules.`,
      }),
    );
  }
}

function renderCookies(report) {
  const container = document.getElementById('scan_cookies');
  container.textContent = '';
  const cookies = report.cookies || [];
  if (cookies.length === 0) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'No session cookies seen.',
      }),
    );
    return;
  }
  for (const cookie of cookies) {
    const row = createEl('div', {className: 'd-flex align-items-center gap-3'});
    row.appendChild(
      createEl('span', {className: 'scan-cookie-name', text: cookie.name}),
    );
    const flags = createEl('span', {className: 'd-flex flex-wrap gap-2'});
    for (const [enabled, label] of [
      [cookie.secure, 'Secure'],
      [cookie.httponly, 'HttpOnly'],
      [Boolean(cookie.samesite), `SameSite: ${cookie.samesite || 'not set'}`],
    ]) {
      flags.appendChild(
        createEl('span', {
          className: enabled ? '' : 'scan-flag-off',
          text: enabled ? label : `no ${label}`,
        }),
      );
    }
    row.appendChild(flags);
    container.appendChild(row);
  }
}

function renderEndpoints(report) {
  const container = document.getElementById('scan_endpoints');
  container.textContent = '';
  const endpoints = report.endpoints || [];
  if (endpoints.length === 0) {
    container.appendChild(
      createEl('p', {
        className: 'text-body-secondary mb-0',
        text: 'No endpoints probed.',
      }),
    );
    return;
  }
  const list = createEl('div', {className: 'd-flex flex-column gap-2'});
  for (const ep of endpoints) {
    const statusClass =
      ep.status_code >= 200 && ep.status_code < 300
        ? 'text-success'
        : ep.status_code >= 400
          ? 'text-danger'
          : 'text-warning';
    const statusText =
      ep.status_code === 0
        ? ep.status_text
        : [ep.status_code, ep.status_text].filter(Boolean).join(' ');
    const row = createEl('div', {className: 'd-flex align-items-center gap-3'});
    row.appendChild(
      createEl('span', {
        className: 'scan-endpoint-status ' + statusClass,
        text: statusText,
      }),
    );
    row.appendChild(createEl('span', {className: 'text-break', text: ep.url}));
    list.appendChild(row);
  }
  container.appendChild(list);
}

function renderReport(report) {
  // The instance could not be probed at all - surface the fatal finding and
  // stop; the summary cards below assume at least one probe answered.
  if (!report.reachable) {
    const findings = document.getElementById('scan_findings');
    findings.textContent = '';
    const reportFinding = (report.findings || [])[0];
    const severity = reportFinding?.severity || 'critical';
    const finding = createEl('div', {
      className: 'scan-finding ' + (SEVERITY_CLASS[severity] || ''),
    });
    finding.appendChild(
      createEl('span', {className: 'scan-finding-badge', text: severity}),
    );
    finding.appendChild(
      createEl('span', {
        className: 'scan-finding-msg',
        text:
          reportFinding?.message ||
          report.error ||
          'No probe received an HTTP response.',
      }),
    );
    findings.appendChild(finding);
    document.getElementById('scan_findings_count').textContent = String(
      report.findings?.length || 1,
    );
    document.getElementById('scan_findings_note').textContent =
      reportFinding?.code || 'unreachable';
    document.getElementById('scan_version').textContent = 'Unknown';
    document.getElementById('scan_https').textContent = report.is_https
      ? 'Yes'
      : 'No';
    document.getElementById('scan_debug').textContent = 'No';
    renderTimings(report);
    renderDatabase(report);
    renderCertificate(report);
    renderModules(report);
    renderCookies(report);
    renderEndpoints(report);
    hide(document.getElementById('scan_security'));
    show(document.getElementById('scan_results'));
    return;
  }

  renderSummary(report);
  renderFindings(report);
  renderChecks(report);
  renderReviewSummary(report);
  showScoreSection();
  renderTimings(report);
  renderDatabase(report);
  renderCertificate(report);
  renderModules(report);
  renderCookies(report);
  renderEndpoints(report);
  show(document.getElementById('scan_results'));
}

function showError(message) {
  const el = document.getElementById('scan_error');
  el.textContent = message;
  show(el);
}

// The security section is only meaningful once at least one probe answered;
// the fatal path surfaces the unreachable finding instead and leaves it hidden.
function showScoreSection() {
  show(document.getElementById('scan_security'));
}

function clearError() {
  hide(document.getElementById('scan_error'));
  document.getElementById('scan_error').textContent = '';
}

function setBusy(busy) {
  const btn = document.getElementById('scan_run_btn');
  btn.disabled = busy;
  if (busy) {
    show(document.getElementById('scan_loading'));
    hide(document.getElementById('scan_results'));
    clearError();
  } else {
    hide(document.getElementById('scan_loading'));
  }
}

async function runScan(url) {
  setBusy(true);
  let res;
  try {
    res = await fetch('/scan/run', {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({url}),
    });
  } catch (err) {
    setBusy(false);
    showError('Failed to reach the server: ' + err.message);
    return;
  }
  setBusy(false);
  if (!res.ok) {
    let message = 'The request was rejected.';
    try {
      const body = await res.json();
      if (body && body.error) {
        message = body.error;
      }
    } catch (_err) {
      /* ignore */
    }
    showError(message);
    return;
  }
  const report = await res.json();
  renderReport(report);
}

function init() {
  const form = document.getElementById('scan_form');
  const input = document.getElementById('scan_url');
  form.addEventListener('submit', ev => {
    ev.preventDefault();
    const url = input.value.trim();
    if (!url) {
      showError('Please enter an Odoo URL.');
      return;
    }
    runScan(url);
  });

  document.getElementById('scan_new_btn').addEventListener('click', () => {
    hide(document.getElementById('scan_results'));
    clearError();
    input.focus();
  });
}

init();
