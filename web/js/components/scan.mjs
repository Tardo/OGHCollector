// Copyright 2026 Alexandre D. Díaz
import {Component, registerComponent} from 'mirlo';
import '@scss/pages/scan.scss';
import {t} from '@app/utils/i18n';

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

// Light DOM (disableShadow): the page's Bootstrap classes and scan.scss apply as is.
class Scan extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      scan_form: {mode: 'id', events: {submit: this.onSubmit}},
      scan_new_btn: {mode: 'id', events: {click: this.onClickNew}},
    });
  }

  onSubmit(ev) {
    ev.preventDefault();
    const url = this.queryId('scan_url').value.trim();
    if (!url) {
      this.showError(t('Please enter an Odoo URL.'));
      return;
    }
    this.runScan(url);
  }

  onClickNew() {
    hide(this.queryId('scan_results'));
    this.clearError();
    this.queryId('scan_url').focus();
  }

  statusLabel(status) {
    switch (status) {
      case 'supported':
        return t('Supported');
      case 'outdated':
        return t('Outdated');
      default:
        return t('Unknown');
    }
  }

  renderSummary(report) {
    const versionEl = this.queryId('scan_version');
    const versionNote = this.queryId('scan_version_note');
    const detected = report.version.detected;
    versionEl.textContent = detected || t('Unknown');
    versionNote.textContent = detected
      ? this.statusLabel(report.version.status)
      : t('source: {0}', report.version.source);

    this.queryId('scan_https').textContent = report.is_https
      ? t('Yes')
      : t('No');
    this.queryId('scan_debug').textContent = report.debug_mode
      ? t('Yes')
      : t('No');

    const findings = report.findings || [];
    this.queryId('scan_findings_count').textContent = String(findings.length);
    const critical = findings.filter(f => f.severity === 'critical').length;
    const high = findings.filter(f => f.severity === 'high').length;
    const findingsNote = this.queryId('scan_findings_note');
    if (critical > 0) {
      findingsNote.textContent = t('{0} critical', critical);
    } else if (high > 0) {
      findingsNote.textContent = t('{0} high severity', high);
    } else {
      findingsNote.textContent = '';
    }
  }

  renderFindings(report) {
    const container = this.queryId('scan_findings');
    container.textContent = '';
    const findings = report.findings || [];
    if (findings.length === 0) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('No critical points detected.'),
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
        createEl('span', {
          className: 'scan-finding-msg',
          text: finding.message,
        }),
      );
      container.appendChild(row);
    }
  }

  renderReviewSummary(report) {
    const card = this.queryId('scan_score_card');
    const valueEl = this.queryId('scan_score_value');
    const ratingEl = this.queryId('scan_score_rating');
    const breakdownEl = this.queryId('scan_score_breakdown');

    const findings = report.findings || [];
    const breakdown = Object.keys(SEVERITY_CLASS)
      .map(severity => [
        severity,
        findings.filter(f => f.severity === severity).length,
      ])
      .filter(([, count]) => count > 0);
    const severity = breakdown[0]?.[0];
    valueEl.textContent = report.reachable
      ? severity || t('None')
      : t('Unknown');
    ratingEl.textContent = t('Highest observed severity');

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

  renderChecks(report) {
    const container = this.queryId('scan_checks');
    container.textContent = '';
    const checks = report.checks || [];
    if (checks.length === 0) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('No security checks ran.'),
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

  renderTimings(report) {
    const container = this.queryId('scan_timings');
    container.textContent = '';
    const tm = report.timings || {};
    const rows = [
      [t('Total scan time'), `${tm.total_ms.toFixed(0)} ms`],
      [t('Main endpoint TTFB'), `${tm.main_ttfb_ms.toFixed(0)} ms`],
      [t('Main endpoint total'), `${tm.main_total_ms.toFixed(0)} ms`],
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

  renderDatabase(report) {
    const container = this.queryId('scan_database');
    container.textContent = '';
    const db = report.database || {};
    if (!report.reachable) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('Instance not reachable.'),
        }),
      );
      return;
    }
    const parts = [];
    if (db.info_available) {
      parts.push(t('info endpoint exposed'));
    }
    if (db.manager_available) {
      parts.push(t('manager exposed'));
    }
    if (parts.length === 0) {
      parts.push(t('no database exposure detected'));
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

  renderCertificate(report) {
    const container = this.queryId('scan_certificate');
    container.textContent = '';
    if (!report.is_https) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('Not applicable: the target uses HTTP.'),
        }),
      );
      return;
    }
    const cert = report.certificate;
    if (!cert) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('Certificate could not be inspected.'),
        }),
      );
      return;
    }
    const trust =
      cert.verified && cert.valid_now
        ? t('trusted and valid')
        : !cert.valid_now
          ? t('expired or not yet valid')
          : cert.self_signed
            ? t('self-signed')
            : t('untrusted or hostname mismatch');
    const rows = [
      [t('Status'), trust],
      [t('Subject'), cert.subject_cn || cert.subject_org || t('not provided')],
      [t('Issuer'), cert.issuer_cn || cert.issuer_org || t('not provided')],
      [
        t('Validity'),
        t(
          '{0} to {1}',
          cert.not_before || t('unknown'),
          cert.not_after || t('unknown'),
        ),
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
          text: t('SAN: {0}', cert.sans.join(', ')),
        }),
      );
    }
  }

  renderModules(report) {
    const container = this.queryId('scan_modules');
    container.textContent = '';
    const modules = report.modules || {};
    if (!report.reachable) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('Instance not reachable.'),
        }),
      );
      return;
    }
    const total = modules.module_count || modules.module_names.length;
    if (modules.page_status !== null && modules.page_status !== undefined) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-2',
          text: t('Enumeration endpoint: HTTP {0}', modules.page_status),
        }),
      );
    }
    if (total === 0) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t(
            'Module enumeration not available (endpoint gated or empty).',
          ),
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
          text: t('Showing the first {0} modules.', total),
        }),
      );
    }
  }

  renderCookies(report) {
    const container = this.queryId('scan_cookies');
    container.textContent = '';
    const cookies = report.cookies || [];
    if (cookies.length === 0) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('No session cookies seen.'),
        }),
      );
      return;
    }
    for (const cookie of cookies) {
      const row = createEl('div', {
        className: 'd-flex align-items-center gap-3',
      });
      row.appendChild(
        createEl('span', {className: 'scan-cookie-name', text: cookie.name}),
      );
      const flags = createEl('span', {className: 'd-flex flex-wrap gap-2'});
      for (const [enabled, label] of [
        [cookie.secure, 'Secure'],
        [cookie.httponly, 'HttpOnly'],
        [
          Boolean(cookie.samesite),
          `SameSite: ${cookie.samesite || t('not set')}`,
        ],
      ]) {
        flags.appendChild(
          createEl('span', {
            className: enabled ? '' : 'scan-flag-off',
            text: enabled ? label : t('no {0}', label),
          }),
        );
      }
      row.appendChild(flags);
      container.appendChild(row);
    }
  }

  renderEndpoints(report) {
    const container = this.queryId('scan_endpoints');
    container.textContent = '';
    const endpoints = report.endpoints || [];
    if (endpoints.length === 0) {
      container.appendChild(
        createEl('p', {
          className: 'text-body-secondary mb-0',
          text: t('No endpoints probed.'),
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
      const row = createEl('div', {
        className: 'd-flex align-items-center gap-3',
      });
      row.appendChild(
        createEl('span', {
          className: 'scan-endpoint-status ' + statusClass,
          text: statusText,
        }),
      );
      row.appendChild(
        createEl('span', {className: 'text-break', text: ep.url}),
      );
      list.appendChild(row);
    }
    container.appendChild(list);
  }

  renderReport(report) {
    // The instance could not be probed at all - surface the fatal finding and
    // stop; the summary cards below assume at least one probe answered.
    if (!report.reachable) {
      const findings = this.queryId('scan_findings');
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
            t('No probe received an HTTP response.'),
        }),
      );
      findings.appendChild(finding);
      this.queryId('scan_findings_count').textContent = String(
        report.findings?.length || 1,
      );
      this.queryId('scan_findings_note').textContent =
        reportFinding?.code || 'unreachable';
      this.queryId('scan_version').textContent = t('Unknown');
      this.queryId('scan_https').textContent = report.is_https ? 'Yes' : 'No';
      this.queryId('scan_debug').textContent = 'No';
      this.renderTimings(report);
      this.renderDatabase(report);
      this.renderCertificate(report);
      this.renderModules(report);
      this.renderCookies(report);
      this.renderEndpoints(report);
      hide(this.queryId('scan_security'));
      show(this.queryId('scan_results'));
      return;
    }

    this.renderSummary(report);
    this.renderFindings(report);
    this.renderChecks(report);
    this.renderReviewSummary(report);
    this.showScoreSection();
    this.renderTimings(report);
    this.renderDatabase(report);
    this.renderCertificate(report);
    this.renderModules(report);
    this.renderCookies(report);
    this.renderEndpoints(report);
    show(this.queryId('scan_results'));
  }

  showError(message) {
    const el = this.queryId('scan_error');
    el.textContent = message;
    show(el);
  }

  // The security section is only meaningful once at least one probe answered;
  // the fatal path surfaces the unreachable finding instead and leaves it hidden.
  showScoreSection() {
    show(this.queryId('scan_security'));
  }

  clearError() {
    hide(this.queryId('scan_error'));
    this.queryId('scan_error').textContent = '';
  }

  setBusy(busy) {
    const btn = this.queryId('scan_run_btn');
    btn.disabled = busy;
    if (busy) {
      show(this.queryId('scan_loading'));
      hide(this.queryId('scan_results'));
      this.clearError();
    } else {
      hide(this.queryId('scan_loading'));
    }
  }

  async runScan(url) {
    this.setBusy(true);
    let res;
    try {
      res = await fetch('/scan/run', {
        method: 'POST',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({url}),
      });
    } catch (err) {
      this.setBusy(false);
      this.showError(t('Failed to reach the server: {0}', err.message));
      return;
    }
    this.setBusy(false);
    if (!res.ok) {
      let message = t('The request was rejected.');
      try {
        const body = await res.json();
        if (body && body.error) {
          message = body.error;
        }
      } catch (_err) {
        /* ignore */
      }
      this.showError(message);
      return;
    }
    const report = await res.json();
    this.renderReport(report);
  }
}

registerComponent('scan', Scan);
