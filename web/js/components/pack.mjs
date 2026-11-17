// Copyright 2026 Alexandre D. Díaz
import {Component, registerComponent, HTTP_METHOD} from 'mirlo';
import '@scss/pages/pack.scss';
import QRCode from 'qrcode';
import {
  addToPack,
  createPack,
  decodeSharedPack,
} from '@app/utils/favorites-store';
import {exportPackZip} from '@app/utils/pack-export';
import {createModuleTileLink} from '@app/utils/module-tile';
import {renderLicenseReview} from '@app/utils/license-review';
import {t} from '@app/utils/i18n';

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
        found
          ? `${mod.org} · ${mod.technical_name}`
          : `${mod.org} · ${t('not found')}`,
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
      ? t('resolved {0} of {1}', found.length, shared.modules.length)
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
  let license_review = null;
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
      const data = await res.json();
      for (const info of data.modules) {
        info_by_key.set(moduleKey(info.org, info.technical_name), info);
      }
      license_review = data.license_review;
    }
  } catch (err) {
    console.error('Failed to resolve pack module info:', err);
  }
  return {info_by_key, license_review};
}

// Light DOM (disableShadow): the page's Bootstrap classes and pack.scss apply
// as is. The pack comes from the `d` query parameter (decodeSharedPack).
class Pack extends Component {
  #shared = null;
  #versions = [];
  #info_by_key = new Map();

  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      pack_copy_link_btn: {mode: 'id', events: {click: this.onClickCopyLink}},
      pack_import_btn: {mode: 'id', events: {click: this.onClickImport}},
      pack_export_btn: {mode: 'id', events: {click: this.onClickExport}},
      // Doubles as the "convert to another version" control: switching it
      // re-resolves module availability/size against the picked version, and
      // Export/Import read its current value.
      pack_export_version: {mode: 'id', events: {change: this.onChangeVersion}},
    });
    Component.useFetchData({
      odoo_versions: {
        endpoint: '/common/odoo/versions',
        method: HTTP_METHOD.GET,
        cache_name: 'odoo_versions',
      },
    });
  }

  // The pack renders without the versions: a failed fetch only logs.
  async onWillStart() {
    try {
      await super.onWillStart(...arguments);
    } catch (err) {
      console.error('Failed to load Odoo versions:', err);
    }
    this.#versions = this.getFetchData('odoo_versions') || [];
  }

  async onStart() {
    super.onStart();
    const encoded = new URLSearchParams(window.location.search).get('d');
    const shared = encoded && decodeSharedPack(encoded);
    if (!shared || shared.modules.length === 0) {
      this.queryId('pack_invalid').classList.remove('d-none');
      return;
    }
    this.#shared = shared;

    this.queryId('pack_title').textContent = shared.name;
    this.queryId('pack_content').classList.remove('d-none');
    this.queryId('pack_share_url').value = window.location.href;
    this.renderQr(window.location.href);

    const version_select = this.queryId('pack_export_version');
    for (const {value} of this.#versions) {
      version_select.add(new Option(value));
    }
    if (this.#versions.some(v => v.value === shared.odoo_version)) {
      version_select.value = shared.odoo_version;
    }
    this.#info_by_key = await this.refresh(version_select.value);
  }

  async onClickCopyLink(ev) {
    const btn = ev.currentTarget;
    const original = btn.textContent;
    try {
      await navigator.clipboard.writeText(window.location.href);
      btn.textContent = t('Copied!');
    } catch {
      this.queryId('pack_share_url').select();
      btn.textContent = t('Press Ctrl+C');
    }
    setTimeout(() => {
      btn.textContent = original;
    }, 1500);
  }

  onClickImport() {
    const shared = this.#shared;
    const name = window.prompt(t('Pack name:'), shared.name)?.trim();
    if (!name) {
      return;
    }
    const pack_version =
      this.queryId('pack_export_version').value || shared.odoo_version;
    const pack = createPack(name, pack_version);
    let skipped = 0;
    for (const mod of shared.modules) {
      const info = this.#info_by_key.get(
        moduleKey(mod.org, mod.technical_name),
      );
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
    let message = t('Added "{0}" to your Packs (see Favorites & Packs).', name);
    if (skipped) {
      message +=
        ' ' +
        t(
          '{0} module(s) not available for Odoo {1} were skipped.',
          skipped,
          pack_version,
        );
    }
    window.alert(message);
  }

  onClickExport(ev) {
    const btn = ev.currentTarget;
    btn.disabled = true;
    exportPackZip(
      this.#shared,
      this.queryId('pack_export_version').value,
    ).finally(() => {
      btn.disabled = false;
    });
  }

  async onChangeVersion(ev) {
    this.#info_by_key = await this.refresh(ev.target.value);
  }

  // Re-resolves every module against `odoo_version` and re-renders: both the
  // initial render and the "convert to another version" action.
  async refresh(odoo_version) {
    const shared = this.#shared;
    this.queryId('pack_subtitle').textContent = odoo_version
      ? t(
          'Shared Odoo module pack — {0} module(s) for Odoo {1}',
          shared.modules.length,
          odoo_version,
        )
      : t('Shared Odoo module pack — {0} module(s)', shared.modules.length);
    const {info_by_key, license_review} = await fetchInfo(
      shared.modules,
      odoo_version,
    );
    renderModuleList(shared, info_by_key);
    renderLicenseReview(this.queryId('pack_license_review'), license_review);
    renderStats(
      shared,
      shared.modules.map(
        m =>
          info_by_key.get(moduleKey(m.org, m.technical_name)) || {found: false},
      ),
    );
    this.queryId('pack_stat_size_note').textContent = odoo_version
      ? t('For Odoo {0}', odoo_version)
      : t('Latest tracked version per module');
    return info_by_key;
  }

  async renderQr(url) {
    const canvas = this.queryId('pack_qr');
    try {
      await QRCode.toCanvas(canvas, url, {width: 220, margin: 1});
    } catch {
      // shortcut: QR byte-mode tops out ~2.9KB, so a pack with many modules
      // can overflow it. The link (always shown) is the primary share
      // channel; the QR is best-effort convenience only.
      canvas.classList.add('d-none');
      this.queryId('pack_qr_unavailable').classList.remove('d-none');
    }
  }
}

registerComponent('pack', Pack);
