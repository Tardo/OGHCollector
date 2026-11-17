// Copyright Alexandre D. Díaz
import {registerComponent} from 'mirlo';
import '@scss/pages/module.scss';
import LazyTabs, {fetchIntoPane} from '@app/components/lazy-tabs';
import {
  addToPack,
  createPack,
  getPacks,
  isFavorite,
  isInPack,
  removeFromPack,
  toggleFavorite,
} from '@app/utils/favorites-store';
import {t} from '@app/utils/i18n';

function badgeSnippet(details, format) {
  const module_url = new URL(details.dataset.modulePath, window.location.origin)
    .href;
  const images = [...details.querySelectorAll('.badge-embed-previews img')];
  if (format === 'rst') {
    return images
      .map(
        ({src, alt}) =>
          `.. image:: ${src}\n   :target: ${module_url}\n   :alt: ${alt}`,
      )
      .join('\n\n');
  }
  if (format === 'html') {
    return images
      .map(
        ({src, alt}) =>
          `<a href="${module_url}"><img src="${src}" alt="${alt}"></a>`,
      )
      .join('\n');
  }
  return images
    .map(({src, alt}) => `[![${alt}](${src})](${module_url})`)
    .join(' ');
}

function refreshBadgeSnippet(details) {
  const format = details.querySelector('.badge-embed-format').value;
  details.querySelector('.badge-embed-code').value = badgeSnippet(
    details,
    format,
  );
}

// Packs are built for one Odoo version (see favorites-store.mjs), so a new
// one needs a version at creation time - use whichever version tab is
// currently active on this page. Returns null on the pull requests tab
// (id="pull-requests", not "version-X"), which isn't a released version.
function getActiveOdooVersion() {
  const pane = document.querySelector('#versions-tabContent .tab-pane.active');
  return pane?.id.match(/^version-(.+)$/)?.[1] ?? null;
}

// Odoo versions this module actually has data for, read off the version
// tabs the server already rendered (id="version-X"; the pull requests tab
// is excluded) - same source getActiveOdooVersion() trusts.
function getAvailableOdooVersions() {
  return new Set(
    [...document.querySelectorAll('#versions-tabContent .tab-pane')]
      .map(el => el.id.match(/^version-(.+)$/)?.[1])
      .filter(Boolean),
  );
}

// Rebuilds the pack dropdown menu (checkmarks the packs this module is
// already in, plus a "New pack" entry) - called on init and after every
// add/remove/create so the list never goes stale while the menu is open.
function buildPackMenuItems(menu, mod, available_versions) {
  menu.textContent = '';
  const packs = getPacks();
  for (const pack of packs) {
    // A pack targets one Odoo version - don't let this module be added to
    // one it doesn't exist for.
    const unavailable =
      pack.odoo_version && !available_versions.has(pack.odoo_version);
    const item = document.createElement('li');
    const link = document.createElement('a');
    link.href = '#';
    link.className =
      'dropdown-item d-flex align-items-center justify-content-between gap-2' +
      (unavailable ? ' disabled' : '');
    if (unavailable) {
      link.setAttribute('aria-disabled', 'true');
      link.title = t('Not available for Odoo {0}', pack.odoo_version);
    }
    const label = document.createElement('span');
    const in_pack = isInPack(pack.id, mod);
    link.setAttribute(
      'aria-label',
      `${in_pack ? 'Remove from' : 'Add to'} ${pack.name}`,
    );
    if (in_pack) {
      const check = document.createElement('span');
      check.className = 'material-icons';
      check.setAttribute('aria-hidden', 'true');
      check.textContent = 'check';
      label.appendChild(check);
    }
    label.append(pack.name);
    link.appendChild(label);
    if (pack.odoo_version) {
      const version_badge = document.createElement('span');
      version_badge.className = 'badge text-bg-light border';
      version_badge.textContent = pack.odoo_version;
      link.appendChild(version_badge);
    }
    link.addEventListener('click', ev => {
      ev.preventDefault();
      if (unavailable) {
        return;
      }
      if (isInPack(pack.id, mod)) {
        removeFromPack(pack.id, mod);
      } else {
        addToPack(pack.id, mod);
      }
      buildPackMenuItems(menu, mod, available_versions);
    });
    item.appendChild(link);
    menu.appendChild(item);
  }
  if (packs.length) {
    const divider = document.createElement('li');
    divider.innerHTML = '<hr class="dropdown-divider">';
    menu.appendChild(divider);
  }
  const new_item = document.createElement('li');
  const new_link = document.createElement('a');
  new_link.href = '#';
  new_link.className = 'dropdown-item';
  new_link.textContent = t('+ New pack…');
  new_link.addEventListener('click', ev => {
    ev.preventDefault();
    const version = getActiveOdooVersion();
    if (!version) {
      window.alert(
        'Select a released Odoo version tab first - packs are built for one version.',
      );
      return;
    }
    const name = window.prompt('Pack name:')?.trim();
    if (!name) {
      return;
    }
    addToPack(createPack(name, version).id, mod);
    buildPackMenuItems(menu, mod, available_versions);
  });
  new_item.appendChild(new_link);
  menu.appendChild(new_item);
}

function initFavoriteActions(container) {
  const mod = {
    org: container.dataset.org,
    technical_name: container.dataset.technicalName,
    name: container.dataset.name,
  };

  const toggle_btn = container.querySelector('.favorite-toggle-btn');
  const icon = toggle_btn.querySelector('.favorite-icon');
  const syncToggleBtn = is_favorite => {
    toggle_btn.classList.toggle('active', is_favorite);
    toggle_btn.setAttribute('aria-pressed', String(is_favorite));
    icon.textContent = is_favorite ? 'star' : 'star_border';
  };
  syncToggleBtn(isFavorite(mod));
  toggle_btn.addEventListener('click', () =>
    syncToggleBtn(toggleFavorite(mod)),
  );

  buildPackMenuItems(
    container.querySelector('.pack-dropdown-menu'),
    mod,
    getAvailableOdooVersions(),
  );
}

// The whole module page (pages/module.html), light DOM: the Odoo-version tabs
// load lazily (LazyTabs) and their panes are handled by delegation on the
// host, since their content arrives after start.
class ModulePage extends LazyTabs {
  getEventDefs() {
    const defs = super.getEventDefs();
    defs[''].events.change = this.onChange;
    defs[''].events.click = this.onClick;
    return defs;
  }

  onStart() {
    super.onStart();
    // `toggle` doesn't bubble: listen in the capture phase.
    this.addEventListener(
      'toggle',
      ev => {
        if (ev.target.matches?.('.badge-embed') && ev.target.open) {
          refreshBadgeSnippet(ev.target);
        }
      },
      true,
    );
    const container = this.query('.module-favorite-actions');
    if (container) {
      initFavoriteActions(container);
    }
  }

  onChange(ev) {
    // Re-fetches just this pane instead of reloading the page - a full-page
    // reload has nowhere to send the selection: only the active Odoo-version
    // tab is server-rendered, so a reload on any other tab would silently
    // drop which version was picked.
    const select = ev.target.closest('.module-version-select');
    const pane = select?.closest('.tab-pane');
    if (pane) {
      const url = `${window.location.pathname}/tab/${encodeURIComponent(select.dataset.odooVersion)}?version=${encodeURIComponent(select.value)}`;
      fetchIntoPane(pane, url);
      return;
    }
    if (ev.target.matches('.badge-embed-format')) {
      refreshBadgeSnippet(ev.target.closest('.badge-embed'));
    } else if (ev.target.matches('[data-pr-filter]')) {
      this.filterPullRequests();
    }
  }

  async onClick(ev) {
    const button = ev.target.closest('.badge-embed-copy');
    if (!button) {
      return;
    }
    const code = button
      .closest('.badge-embed')
      .querySelector('.badge-embed-code');
    try {
      // Needs a secure context (https/localhost); select the text otherwise.
      await navigator.clipboard.writeText(code.value);
      button.textContent = t('Copied!');
    } catch {
      code.select();
      button.textContent = t('Press Ctrl+C');
    }
    setTimeout(() => {
      button.textContent = t('Copy');
    }, 1500);
  }

  // Pull requests tab: filters on the items' data attributes. The server
  // already orders them (newest version, then the best candidate first) and
  // renders the initial count.
  filterPullRequests() {
    const pr_pane = this.queryId('pull-requests');
    const filters = [...pr_pane.querySelectorAll('[data-pr-filter]')];
    let shown = 0;
    const items = pr_pane.querySelectorAll('.pr-item');
    for (const item of items) {
      const match = filters.every(
        f => !f.value || item.dataset[f.dataset.prFilter] === f.value,
      );
      item.hidden = !match;
      shown += match ? 1 : 0;
    }
    pr_pane.querySelector('.pr-filter-count').textContent = t(
      '{0} of {1}',
      shown,
      items.length,
    );
    pr_pane.querySelector('.pr-empty').hidden = shown > 0;
  }
}

registerComponent('module', ModulePage);
