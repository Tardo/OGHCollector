// Copyright Alexandre D. Díaz
import {registerComponent} from 'mirlo';
import SearchDropdown from './search-dropdown.mjs';
import '@scss/components/module-search.scss';

class ModuleSearch extends SearchDropdown {
  get searchEndpoint() {
    return '/common/odoo/module/list';
  }

  normalizeQuery(query) {
    return query.replaceAll(' ', '_').toLowerCase();
  }

  searchKey(module) {
    return module.technical_name;
  }

  createResultItem(module) {
    const item_container = document.createElement('li');
    const item = document.createElement('a');
    item.classList.add('item');
    item.href = `/module/${module.org_name}/${module.technical_name}`;
    item.innerHTML = `<div>${module.technical_name}</div><div class="info">${module.org_name.toUpperCase()}: ${module.versions.join(' - ')}<div>`;
    item_container.appendChild(item);
    return item_container;
  }
}

registerComponent('module-search', ModuleSearch);
