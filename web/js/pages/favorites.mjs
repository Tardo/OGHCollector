// Copyright 2026 Alexandre D. Díaz
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

// Populated once on init - every pack card's version <select> shares it, no
// need to refetch on every rename/delete-triggered re-render.
let odoo_versions = [];

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
  remove_btn.title = `Remove ${mod.name || mod.technical_name}`;
  remove_btn.setAttribute(
    'aria-label',
    `Remove ${mod.name || mod.technical_name}`,
  );
  remove_btn.textContent = '×';
  remove_btn.addEventListener('click', () => onRemove(mod));
  tile.appendChild(remove_btn);

  return tile;
}

function renderStats() {
  const packs = getPacks();
  document.getElementById('stat_favorites').textContent = getFavorites().length;
  document.getElementById('stat_packs').textContent = packs.length;
  document.getElementById('stat_pack_modules').textContent = packs.reduce(
    (sum, pack) => sum + pack.modules.length,
    0,
  );
}

function renderFavorites() {
  const list = document.getElementById('favorites_list');
  const empty = document.getElementById('favorites_empty');
  const favorites = getFavorites();
  list.textContent = '';
  empty.classList.toggle('d-none', favorites.length > 0);
  for (const mod of favorites) {
    list.appendChild(
      createModuleTile(mod, removed => {
        removeFavorite(removed);
        renderFavorites();
        renderStats();
      }),
    );
  }
}

function createPackCard(pack) {
  const card = document.createElement('div');
  card.className = 'card pack-card';

  const header = document.createElement('div');
  header.className =
    'card-header d-flex align-items-center justify-content-between gap-3';

  const title = document.createElement('span');
  title.className = 'pack-card-title fw-semibold';
  const title_icon = document.createElement('span');
  title_icon.className = 'pack-card-icon';
  title_icon.setAttribute('aria-hidden', 'true');
  title_icon.textContent = '\u{1F4E6}';
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
  const version = pack.odoo_version || odoo_versions[0]?.value;
  if (version) {
    const version_badge = document.createElement('span');
    version_badge.className = 'badge text-bg-light border';
    version_badge.title = 'Odoo version this pack was built for';
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
    export_btn.textContent = 'Export';
    export_btn.title = `Export for Odoo ${version} — open the pack's share link to convert it to another version first`;
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
    share_btn.textContent = 'Share';
    share_btn.title = 'Open a shareable link (and QR code) for this pack';
    share_btn.addEventListener('click', () => {
      window.open(`/pack?d=${encodePackForShare(pack)}`, '_blank', 'noopener');
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
  more_btn.title = 'More actions';
  more_btn.innerHTML =
    '<span aria-hidden="true">⋮</span><span class="visually-hidden">More actions</span>';
  more_wrap.appendChild(more_btn);

  const menu = document.createElement('ul');
  menu.className = 'dropdown-menu dropdown-menu-end';

  const rename_item = document.createElement('li');
  const rename_btn = document.createElement('button');
  rename_btn.type = 'button';
  rename_btn.className = 'dropdown-item';
  rename_btn.textContent = 'Rename';
  rename_btn.addEventListener('click', () => {
    const name = window.prompt('Pack name:', pack.name)?.trim();
    if (!name) {
      return;
    }
    renamePack(pack.id, name);
    renderPacks();
  });
  rename_item.appendChild(rename_btn);
  menu.appendChild(rename_item);

  const delete_item = document.createElement('li');
  const delete_btn = document.createElement('button');
  delete_btn.type = 'button';
  delete_btn.className = 'dropdown-item text-danger';
  delete_btn.textContent = 'Delete pack';
  delete_btn.addEventListener('click', () => {
    if (!window.confirm(`Delete pack "${pack.name}"?`)) {
      return;
    }
    deletePack(pack.id);
    renderPacks();
    renderStats();
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
          renderPacks();
          renderStats();
        }),
      );
    }
  }
  card.appendChild(body);

  return card;
}

function renderPacks() {
  const container = document.getElementById('packs_list');
  const empty = document.getElementById('packs_empty');
  const packs = getPacks();
  container.textContent = '';
  empty.classList.toggle('d-none', packs.length > 0);
  for (const pack of packs) {
    container.appendChild(createPackCard(pack));
  }
}

document.getElementById('new_pack_btn').addEventListener('click', () => {
  const version_select = document.getElementById('new_pack_version');
  if (!version_select.value) {
    window.alert('Still loading Odoo versions - try again in a second.');
    return;
  }
  const name = window.prompt('Pack name:')?.trim();
  if (!name) {
    return;
  }
  createPack(name, version_select.value);
  renderPacks();
  renderStats();
});

renderFavorites();
renderPacks();
renderStats();

fetch('/common/odoo/versions')
  .then(res => (res.ok ? res.json() : []))
  .then(versions => {
    odoo_versions = versions;
    const version_select = document.getElementById('new_pack_version');
    for (const {value} of versions) {
      version_select.add(new Option(value));
    }
    renderPacks();
  })
  .catch(err => console.error('Failed to load Odoo versions:', err));
