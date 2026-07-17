// Copyright Alexandre D. Díaz
import '@app/components/module-search';
import '@scss/pages/module.scss';
import {fetchIntoPane, initLazyTabPanes} from '@app/utils/lazy-tab';
import {bindSearchModal} from '@app/utils/search-modal';
import {
  addToPack,
  createPack,
  getPacks,
  isFavorite,
  isInPack,
  removeFromPack,
  toggleFavorite,
} from '@app/utils/favorites-store';

bindSearchModal('module_search', 'mirlo-module-search');

// Delegated (not queried once at load) since lazy-loaded tabs inject their
// own `.module-version-select` after this script has already run. Re-fetches
// just this pane instead of reloading the page - a full-page reload has
// nowhere to send the selection: only the active Odoo-version tab is
// server-rendered (see pages/module.html), and there's no tab activated
// from the URL, so a reload on any other tab would silently drop which
// version was picked.
document
  .getElementById('versions-tabContent')
  ?.addEventListener('change', ev => {
    const select = ev.target.closest('.module-version-select');
    const pane = select?.closest('.tab-pane');
    if (!pane) {
      return;
    }
    const url = `${window.location.pathname}/tab/${encodeURIComponent(select.dataset.odooVersion)}?version=${encodeURIComponent(select.value)}`;
    fetchIntoPane(pane, url);
  });

// Odoo-version tabs beyond the active one are rendered empty by the server;
// fetch their content on first activation so a module tracked across many
// versions doesn't ship every version's full code analysis on initial load.
initLazyTabPanes('versions-tab');

// Packs are built for one Odoo version (see favorites-store.mjs), so a new
// one needs a version at creation time - use whichever version tab is
// currently active on this page. Returns null on the PR tabs (id="pr-N",
// not "version-X"), which don't correspond to a released version.
function getActiveOdooVersion() {
  const pane = document.querySelector('#versions-tabContent .tab-pane.active');
  return pane?.id.match(/^version-(.+)$/)?.[1] ?? null;
}

// Odoo versions this module actually has data for, read off the version
// tabs the server already rendered (id="version-X", PR tabs use "pr-N" and
// are excluded) - same source getActiveOdooVersion() trusts.
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
      link.title = `Not available for Odoo ${pack.odoo_version}`;
    }
    const label = document.createElement('span');
    label.textContent = `${isInPack(pack.id, mod) ? '✓ ' : ''}${pack.name}`;
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
  new_link.textContent = '+ New pack…';
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

function initFavoriteActions() {
  const container = document.querySelector('.module-favorite-actions');
  if (!container) {
    return;
  }
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
    icon.textContent = is_favorite ? '★' : '☆';
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

initFavoriteActions();
