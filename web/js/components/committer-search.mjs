// Copyright Alexandre D. Díaz
import {registerComponent} from 'mirlo';
import SearchDropdown from './search-dropdown.mjs';
import '@scss/components/committer-search.scss';

class CommitterSearch extends SearchDropdown {
  get searchEndpoint() {
    return '/common/odoo/committer/list';
  }

  searchKey(committer) {
    return committer.name;
  }

  createResultItem(committer) {
    const item_container = document.createElement('li');
    const item = document.createElement('a');
    item.classList.add('item');
    item.href = `/committer/${encodeURIComponent(committer.name)}`;
    item.innerHTML = `<div>${committer.name}</div><div class="info">${committer.total_commits} commits - ${committer.modules_touched} modules</div>`;
    item_container.appendChild(item);
    return item_container;
  }
}

registerComponent('committer-search', CommitterSearch);
