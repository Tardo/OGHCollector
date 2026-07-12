// Copyright 2026 Alexandre D. Díaz
import '@scss/pages/pack.scss';
import QRCode from 'qrcode';
import {
  addToPack,
  createPack,
  decodeSharedPack,
} from '@app/utils/favorites-store';
import {exportPackZip} from '@app/utils/pack-export';
import {createModuleTileLink} from '@app/utils/module-tile';

function moduleKey(org, technical_name) {
  return `${org}/${technical_name}`;
}

function renderModuleList(shared, info_by_key) {
  const container = document.getElementById('pack_module_list');
  container.textContent = '';
  for (const mod of shared.modules) {
    const info = info_by_key.get(moduleKey(mod.org, mod.technical_name));
    const found = !info || info.found;
    const tile = document.createElement('div');
    tile.className = found ? 'module-tile' : 'module-tile module-tile-notfound';
    tile.appendChild(
      createModuleTileLink(
        mod,
        (info?.found && info.name) || mod.technical_name,
        found ? `${mod.org} · ${mod.technical_name}` : `${mod.org} · not found`,
      ),
    );
    container.appendChild(tile);
  }
}

function renderStats(shared, resolved) {
  const found = resolved.filter(r => r.found);
  document.getElementById('pack_stat_modules').textContent =
    shared.modules.length;
  document.getElementById('pack_stat_modules_note').textContent =
    found.length < shared.modules.length
      ? `resolved ${found.length} of ${shared.modules.length}`
      : '';

  const total_bytes = found.reduce((sum, r) => sum + (r.folder_size || 0), 0);
  document.getElementById('pack_stat_size').textContent =
    `${(total_bytes / 1048576).toFixed(2)} MB`;

  document.getElementById('pack_stat_orgs').textContent = new Set(
    shared.modules.map(m => m.org),
  ).size;
  document.getElementById('pack_stat_repos').textContent = new Set(
    found.map(r => r.repository).filter(Boolean),
  ).size;
}

async function fetchInfo(modules, odoo_version) {
  const info_by_key = new Map();
  try {
    const url = odoo_version
      ? `/pack/info?odoo_version=${encodeURIComponent(odoo_version)}`
      : '/pack/info';
    const res = await fetch(url, {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify(modules),
    });
    if (res.ok) {
      for (const info of await res.json()) {
        info_by_key.set(moduleKey(info.org, info.technical_name), info);
      }
    }
  } catch (err) {
    console.error('Failed to resolve pack module info:', err);
  }
  return info_by_key;
}

// Re-resolves every module against `odoo_version` and re-renders - this is
// both the initial render and the "convert to another version" action (see
// the version <select> wired up in init below).
async function refresh(shared, odoo_version) {
  document.getElementById('pack_subtitle').textContent =
    `Shared Odoo module pack — ${shared.modules.length} module${shared.modules.length === 1 ? '' : 's'}` +
    (odoo_version ? ` for Odoo ${odoo_version}` : '');
  const info_by_key = await fetchInfo(shared.modules, odoo_version);
  renderModuleList(shared, info_by_key);
  renderStats(
    shared,
    shared.modules.map(
      m =>
        info_by_key.get(moduleKey(m.org, m.technical_name)) || {found: false},
    ),
  );
  document.getElementById('pack_stat_size_note').textContent = odoo_version
    ? `For Odoo ${odoo_version}`
    : 'Latest tracked version per module';
  return info_by_key;
}

async function renderQr(url) {
  const canvas = document.getElementById('pack_qr');
  try {
    await QRCode.toCanvas(canvas, url, {width: 220, margin: 1});
  } catch {
    // ponytail: QR byte-mode tops out ~2.9KB: a pack with many modules can
    // overflow it. The link (always shown) is the primary share channel;
    // the QR is best-effort convenience only.
    canvas.classList.add('d-none');
    document.getElementById('pack_qr_unavailable').classList.remove('d-none');
  }
}

async function init() {
  const encoded = new URLSearchParams(window.location.search).get('d');
  const shared = encoded && decodeSharedPack(encoded);
  if (!shared || shared.modules.length === 0) {
    document.getElementById('pack_invalid').classList.remove('d-none');
    return;
  }

  document.getElementById('pack_title').textContent = shared.name;
  document.getElementById('pack_content').classList.remove('d-none');

  const share_url = window.location.href;
  document.getElementById('pack_share_url').value = share_url;
  document
    .getElementById('pack_copy_link_btn')
    .addEventListener('click', async ev => {
      await navigator.clipboard.writeText(share_url);
      const btn = ev.currentTarget;
      const original = btn.textContent;
      btn.textContent = 'Copied!';
      setTimeout(() => {
        btn.textContent = original;
      }, 1500);
    });
  renderQr(share_url);

  let info_by_key = new Map();
  document.getElementById('pack_import_btn').addEventListener('click', () => {
    const name = window.prompt('Pack name:', shared.name)?.trim();
    if (!name) {
      return;
    }
    const pack_version = version_select.value || shared.odoo_version;
    const pack = createPack(name, pack_version);
    let skipped = 0;
    for (const mod of shared.modules) {
      const info = info_by_key.get(moduleKey(mod.org, mod.technical_name));
      if (!info?.found) {
        skipped += 1;
        continue;
      }
      addToPack(pack.id, {
        org: mod.org,
        technical_name: mod.technical_name,
        name: info.name,
      });
    }
    window.alert(
      `Added "${name}" to your Packs (see Favorites & Packs).` +
        (skipped
          ? ` ${skipped} module${skipped === 1 ? '' : 's'} not available for Odoo ${pack_version} were skipped.`
          : ''),
    );
  });

  // Doubles as the "convert to another version" control: switching it
  // re-resolves module availability/size against the picked version (see
  // refresh() above) and Export/Import below both read its current value.
  const version_select = document.getElementById('pack_export_version');
  fetch('/common/odoo/versions')
    .then(res => (res.ok ? res.json() : []))
    .then(async versions => {
      for (const {value} of versions) {
        version_select.add(new Option(value));
      }
      if (versions.some(v => v.value === shared.odoo_version)) {
        version_select.value = shared.odoo_version;
      }
      info_by_key = await refresh(shared, version_select.value);
    })
    .catch(err => console.error('Failed to load Odoo versions:', err));
  version_select.addEventListener('change', async () => {
    info_by_key = await refresh(shared, version_select.value);
  });
  document.getElementById('pack_export_btn').addEventListener('click', ev => {
    ev.currentTarget.disabled = true;
    exportPackZip(shared, version_select.value).finally(() => {
      ev.currentTarget.disabled = false;
    });
  });
}

init();
