// Copyright 2026 Alexandre D. Díaz

// Renders a license review (oghutils::license::LicenseReport, served by the
// dependency resolver, /pack/info and /v1/license-check). Plain DOM, no
// Bootstrap classes: it also renders inside the Doodba components' closed
// shadow roots - see web/scss/components/_license-review.scss.
const SEVERITIES = ['error', 'warning', 'info'];

function paragraph(className, text) {
  const el = document.createElement('p');
  el.className = className;
  el.textContent = text;
  return el;
}

export function renderLicenseReview(container, review) {
  container.textContent = '';
  if (!review) {
    return;
  }
  const findings = review.findings || [];
  const worst = SEVERITIES.find(s => findings.some(f => f.severity === s));

  const section = document.createElement('section');
  section.className = 'license-review';
  section.dataset.worst = worst || 'ok';

  const head = document.createElement('div');
  head.className = 'license-review-head';
  const icon = document.createElement('span');
  icon.className = 'material-icons';
  icon.setAttribute('aria-hidden', 'true');
  icon.textContent = worst ? 'gavel' : 'verified';
  const title = document.createElement('strong');
  title.textContent = 'License review';
  head.append(icon, title);
  if (review.effective_license) {
    const effective = document.createElement('span');
    effective.className = 'license-review-effective';
    const name = document.createElement('b');
    name.textContent = review.effective_license;
    effective.append('Combined work: ', name);
    if (review.effective_license_source) {
      effective.append(` (via ${review.effective_license_source})`);
    }
    head.append(effective);
  }
  section.append(head);

  const counts = Object.entries(review.licenses || {});
  const total = counts.reduce((sum, [, count]) => sum + count, 0);
  if (findings.length) {
    const list = document.createElement('ul');
    list.className = 'license-review-findings';
    for (const finding of findings) {
      const item = document.createElement('li');
      item.dataset.severity = finding.severity;
      const severity = document.createElement('span');
      severity.className = 'license-review-sev';
      severity.textContent = finding.severity;
      item.append(severity, ' ', finding.message);
      list.append(item);
    }
    section.append(list);
  } else if (total) {
    section.append(
      paragraph(
        'license-review-ok',
        `No license conflicts found across ${total} module${total === 1 ? '' : 's'}.`,
      ),
    );
  }
  if (review.proprietary?.length) {
    section.append(
      paragraph(
        'license-review-proprietary',
        `Commercial licenses needed: ${review.proprietary.join(', ')}.`,
      ),
    );
  }
  if (counts.length) {
    section.append(
      paragraph(
        'license-review-counts',
        counts.map(([license, count]) => `${license} × ${count}`).join(' · '),
      ),
    );
  }
  section.append(
    paragraph(
      'license-review-note',
      'Based on the manifest licenses of these modules and everything they depend on: a heuristic to spot problems early, not legal advice.',
    ),
  );
  container.append(section);
}
