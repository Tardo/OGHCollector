// Copyright Alexandre D. Díaz
import {registerComponent} from 'mirlo';
import SearchDropdown from './search-dropdown.mjs';
import '@scss/components/module-search.scss';

class ModuleSearch extends SearchDropdown {
  #el_field = null;
  #el_version = null;

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
