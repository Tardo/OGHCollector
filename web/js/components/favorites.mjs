// Copyright 2026 Alexandre D. Díaz
import {Component, registerComponent, HTTP_METHOD} from 'mirlo';
import '@scss/pages/favorites.scss';
import {
  deletePack,
  encodePackForShare,
  getFavorites,
  getPacks,
  removeFavorite,
  removeFromPack,
  createPack,
  renamePack,
} from '@app/utils/favorites-store';
import {exportPackZip} from '@app/utils/pack-export';
import {createModuleTileLink} from '@app/utils/module-tile';
import {t} from '@app/utils/i18n';

function createModuleTile(mod, onRemove) {
  const tile = document.createElement('div');
  tile.className = 'module-tile';
  tile.appendChild(
    createModuleTileLink(
      mod,
      mod.name || mod.technical_name,
      `${mod.org} · ${mod.technical_name}`,
    ),
  );

  const remove_btn = document.createElement('button');
  remove_btn.type = 'button';
  remove_btn.className = 'module-tile-remove';
  remove_btn.title = t('Remove {0}', mod.name || mod.technical_name);
  remove_btn.setAttribute(
    'aria-label',
    t('Remove {0}', mod.name || mod.technical_name),
  );
  remove_btn.classList.add('material-icons');
  remove_btn.textContent = 'close';
  remove_btn.addEventListener('click', () => onRemove(mod));
  tile.appendChild(remove_btn);

  return tile;
}

// Light DOM (disableShadow): the page's Bootstrap classes and favorites.scss
// apply as is. Favorites and packs live in localStorage (favorites-store).
class Favorites extends Component {
  // Packs created before the odoo_version field existed fall back to the first
  // known version; every pack card shares this list.
  #odoo_versions = [];

  onSetup() {
    Component.disableShadow();
    Component.useStateBinds({
      favorites: {id: 'stat_favorites'},
      packs: {id: 'stat_packs'},
      pack_modules: {id: 'stat_pack_modules'},
    });
    Component.useEvents({
      new_pack_btn: {mode: 'id', events: {click: this.onClickNewPack}},
    });
    Component.useFetchData({
      odoo_versions: {
        endpoint: '/common/odoo/versions',
        method: HTTP_METHOD.GET,
        cache_name: 'odoo_versions',
      },
    });
  }

  // The lists work without the versions: a failed fetch only logs.
  async onWillStart() {
    try {
      await super.onWillStart(...arguments);
    } catch (err) {
      console.error('Failed to load Odoo versions:', err);
    }
    this.#odoo_versions = this.getFetchData('odoo_versions') || [];
  }

  onStart() {
    super.onStart();
    const version_select = this.queryId('new_pack_version');
    for (const {value} of this.#odoo_versions) {
      version_select.add(new Option(value));
    }
    this.renderFavorites();
    this.renderPacks();
    this.renderStats();
  }

  onClickNewPack() {
    const version_select = this.queryId('new_pack_version');
    if (!version_select.value) {
      window.alert(t('Still loading Odoo versions - try again in a second.'));
      return;
    }
    const name = window.prompt('Pack name:')?.trim();
    if (!name) {
      return;
    }
    createPack(name, version_select.value);
    this.renderPacks();
    this.renderStats();
  }

  renderStats() {
    const packs = getPacks();
    this.mirlo.state.favorites = getFavorites().length;
    this.mirlo.state.packs = packs.length;
    this.mirlo.state.pack_modules = packs.reduce(
      (sum, pack) => sum + pack.modules.length,
      0,
    );
  }

  renderFavorites() {
    const list = this.queryId('favorites_list');
    const empty = this.queryId('favorites_empty');
    const favorites = getFavorites();
    list.textContent = '';
    empty.classList.toggle('d-none', favorites.length > 0);
    for (const mod of favorites) {
      list.appendChild(
        createModuleTile(mod, removed => {
          removeFavorite(removed);
          this.renderFavorites();
          this.renderStats();
        }),
      );
    }
  }

  createPackCard(pack) {
    const card = document.createElement('div');
    card.className = 'card pack-card';

    const header = document.createElement('div');
    header.className =
      'card-header d-flex align-items-center justify-content-between gap-3';

    const title = document.createElement('span');
    title.className = 'pack-card-title fw-semibold';
    const title_icon = document.createElement('span');
    title_icon.className = 'pack-card-icon material-icons';
    title_icon.setAttribute('aria-hidden', 'true');
    title_icon.textContent = 'inventory_2';
    title.appendChild(title_icon);
    const title_name = document.createElement('span');
    title_name.className = 'pack-card-name';
    title_name.textContent = pack.name;
    title.appendChild(title_name);
    const count = document.createElement('span');
    count.className = 'badge text-bg-secondary';
    count.textContent = pack.modules.length;
    title.appendChild(count);
    // Packs created before this field existed have no odoo_version - fall
    // back to the first known version rather than leaving Export broken.
    const version = pack.odoo_version || this.#odoo_versions[0]?.value;
    if (version) {
      const version_badge = document.createElement('span');
      version_badge.className = 'badge text-bg-light border';
      version_badge.title = t('Odoo version this pack was built for');
      version_badge.textContent = version;
      title.appendChild(version_badge);
    }
    header.appendChild(title);

    const actions = document.createElement('div');
    actions.className = 'd-flex align-items-center gap-3 flex-shrink-0';

    if (pack.modules.length > 0 && version) {
      const export_btn = document.createElement('button');
      export_btn.type = 'button';
      export_btn.className = 'btn btn-sm btn-outline-primary';
      export_btn.textContent = t('Export');
      export_btn.title = t(
        "Export for Odoo {0} — open the pack's share link to convert it to another version first",
        version,
      );
      export_btn.addEventListener('click', () => {
        export_btn.disabled = true;
        exportPackZip(pack, version).finally(() => {
          export_btn.disabled = false;
        });
      });
      actions.appendChild(export_btn);
    }

    if (pack.modules.length > 0) {
      const share_btn = document.createElement('button');
      share_btn.type = 'button';
      share_btn.className = 'btn btn-sm btn-outline-primary';
      share_btn.textContent = t('Share');
      share_btn.title = t('Open a shareable link (and QR code) for this pack');
      share_btn.addEventListener('click', () => {
        window.open(
          `/pack?d=${encodePackForShare(pack)}`,
          '_blank',
          'noopener',
        );
      });
      actions.appendChild(share_btn);
    }

    // Rename/delete are infrequent - tuck them behind a kebab menu so the
    // header doesn't read as a wall of buttons next to Export/Share.
    const more_wrap = document.createElement('div');
    more_wrap.className = 'dropdown';
    const more_btn = document.createElement('button');
    more_btn.type = 'button';
    more_btn.className = 'btn btn-sm btn-outline-secondary pack-card-more';
    more_btn.setAttribute('data-bs-toggle', 'dropdown');
    more_btn.setAttribute('aria-expanded', 'false');
    more_btn.title = t('More actions');
    more_btn.innerHTML = `<span class="material-icons" aria-hidden="true">more_vert</span><span class="visually-hidden">${t('More actions')}</span>`;
    more_wrap.appendChild(more_btn);

    const menu = document.createElement('ul');
    menu.className = 'dropdown-menu dropdown-menu-end';

    const rename_item = document.createElement('li');
    const rename_btn = document.createElement('button');
    rename_btn.type = 'button';
    rename_btn.className = 'dropdown-item';
    rename_btn.textContent = t('Rename');
    rename_btn.addEventListener('click', () => {
      const name = window.prompt('Pack name:', pack.name)?.trim();
      if (!name) {
        return;
      }
      renamePack(pack.id, name);
      this.renderPacks();
    });
    rename_item.appendChild(rename_btn);
    menu.appendChild(rename_item);

    const delete_item = document.createElement('li');
    const delete_btn = document.createElement('button');
    delete_btn.type = 'button';
    delete_btn.className = 'dropdown-item text-danger';
    delete_btn.textContent = t('Delete pack');
    delete_btn.addEventListener('click', () => {
      if (!window.confirm(t('Delete pack "{0}"?', pack.name))) {
        return;
      }
      deletePack(pack.id);
      this.renderPacks();
      this.renderStats();
    });
    delete_item.appendChild(delete_btn);
    menu.appendChild(delete_item);

    more_wrap.appendChild(menu);
    actions.appendChild(more_wrap);

    header.appendChild(actions);
    card.appendChild(header);

    const body = document.createElement('div');
    body.className = 'card-body';
    if (pack.modules.length === 0) {
      body.innerHTML =
        '<p class="text-body-secondary mb-0">No modules in this pack yet.</p>';
    } else {
      body.className += ' module-tile-grid';
      for (const mod of pack.modules) {
        body.appendChild(
          createModuleTile(mod, removed => {
            removeFromPack(pack.id, removed);
            this.renderPacks();
            this.renderStats();
          }),
        );
      }
    }
    card.appendChild(body);

    return card;
  }

  renderPacks() {
    const container = this.queryId('packs_list');
    const empty = this.queryId('packs_empty');
    const packs = getPacks();
    container.textContent = '';
    empty.classList.toggle('d-none', packs.length > 0);
    for (const pack of packs) {
      container.appendChild(this.createPackCard(pack));
    }
  }
}

registerComponent('favorites', Favorites);
