// Copyright Alexandre D. Díaz
import {registerComponent} from 'mirlo';
import SearchDropdown from './search-dropdown.mjs';
import '@scss/components/module-search.scss';

// Semantic queries hit the server (embedding inference per request), so they
// debounce longer than the local-index search and are best typed as full
// phrases, not prefixes.
const SEMANTIC_DEBOUNCE_MS = 450;

class ModuleSearch extends SearchDropdown {
  #el_field = null;
  #el_version = null;
  #semantic_timer = null;
  #semantic_seq = 0;

  get searchEndpoint() {
    return '/common/odoo/module/list';
  }

  async onWillStart() {
    await super.onWillStart(...arguments);
    this.#el_field = this.queryId('field');
    this.#el_version = this.queryId('version');
    const versions = new Set();
    for (const module of this.getFetchData('records')) {
      for (const version of module.versions) {
        versions.add(version);
      }
    }
    for (const version of [...versions].sort(
      (a, b) => parseFloat(b) - parseFloat(a),
    )) {
      const el_option = document.createElement('option');
      el_option.value = version;
      el_option.textContent = version;
      this.#el_version.appendChild(el_option);
    }
  }

  getEventDefs() {
    return {
      ...super.getEventDefs(),
      field: {mode: 'id', events: {change: this.onChangeFilters}},
      version: {mode: 'id', events: {change: this.onChangeFilters}},
    };
  }

  onChangeFilters() {
    const field_label = this.#el_field.selectedOptions[0].text.toLowerCase();
    this.queryId('search').placeholder = `Search module (${field_label})...`;
    this.refreshResults();
  }

  get isSemantic() {
    return this.#el_field.value === 'semantic';
  }

  onInputSearch(ev) {
    if (!this.isSemantic) {
      super.onInputSearch(ev);
      return;
    }
    clearTimeout(this.#semantic_timer);
    const query = ev.target.value.trim();
    if (query === '') {
      this.fillResults();
      return;
    }
    this.#semantic_timer = setTimeout(
      () => this.#semanticSearch(query),
      SEMANTIC_DEBOUNCE_MS,
    );
  }

  refreshResults() {
    if (!this.isSemantic) {
      super.refreshResults();
      return;
    }
    const query = this.queryId('search').value.trim();
    if (query === '') {
      this.fillResults();
    } else {
      this.#semanticSearch(query);
    }
  }

  async #semanticSearch(query) {
    const seq = ++this.#semantic_seq;
    const params = new URLSearchParams({q: query, limit: '50'});
    if (this.#el_version.value !== '') {
      params.set('odoo_version', this.#el_version.value);
    }
    let rows;
    try {
      const response = await fetch(`/v1/semantic-search?${params}`);
      rows = await response.json();
    } catch {
      rows = [];
    }
    if (seq !== this.#semantic_seq) {
      // A newer query resolved (or was typed) meanwhile - drop this response.
      return;
    }
    // The endpoint returns one row per Odoo version, best score first - fold
    // them into one entry per (org, module); later rows of the same module
    // only contribute their version. `semantic` is the snippet
    // createResultItem picks up via the field name.
    const by_module = new Map();
    for (const row of rows) {
      const key = `${row.org_name}/${row.technical_name}`;
      const entry = by_module.get(key);
      if (entry) {
        entry.versions.push(row.odoo_version);
      } else {
        by_module.set(key, {
          technical_name: row.technical_name,
          org_name: row.org_name,
          versions: [row.odoo_version],
          semantic: `${row.name} · ${row.category} · ${Math.round(row.score * 100)}%`,
        });
      }
    }
    this.fillResults([...by_module.values()]);
  }

  // Technical names use underscores, not spaces - only worth folding for
  // that field, everything else (name, description...) is free text.
  normalizeQuery(query) {
    const q = query.toLowerCase();
    return this.#el_field.value === 'technical_name'
      ? q.replaceAll(' ', '_')
      : q;
  }

  recordMatchesFilters(module) {
    const version = this.#el_version.value;
    return version === '' || module.versions.includes(version);
  }

  searchKey(module) {
    return module[this.#el_field.value] ?? '';
  }

  createResultItem(module) {
    const item_container = document.createElement('li');
    const item = document.createElement('a');
    item.classList.add('item');
    item.href = `/module/${module.org_name}/${module.technical_name}`;

    const el_text = document.createElement('div');
    el_text.classList.add('item-text');
    el_text.innerHTML = `<div>${module.technical_name}</div><div class="info">${module.org_name.toUpperCase()}: ${module.versions.join(' - ')}</div>`;
    const field = this.#el_field.value;
    if (field !== 'technical_name') {
      const snippet = module[field]?.replace(/\s+/g, ' ').trim();
      if (snippet) {
        const el_snippet = document.createElement('div');
        el_snippet.classList.add('snippet');
        el_snippet.textContent =
          snippet.length > 160 ? `${snippet.slice(0, 160)}…` : snippet;
        el_text.appendChild(el_snippet);
      }
    }
    item.appendChild(el_text);

    const el_icon = document.createElement('img');
    el_icon.classList.add('item-icon');
    el_icon.loading = 'lazy';
    el_icon.alt = '';
    // 404s silently for modules without an icon file - just drop the <img>.
    el_icon.onerror = () => el_icon.remove();
    el_icon.src = `/common/odoo/module/${encodeURIComponent(module.org_name)}/${encodeURIComponent(module.technical_name)}/icon`;
    item.appendChild(el_icon);

    item_container.appendChild(item);
    return item_container;
  }
}

registerComponent('module-search', ModuleSearch);
