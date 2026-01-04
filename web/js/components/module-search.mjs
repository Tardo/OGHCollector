// Copyright Alexandre D. Díaz
import {Component, registerComponent, HTTP_METHOD} from 'mirlo';
import '@scss/components/module-search.scss';

const MAX_RESULTS = 50;

class ModuleSearch extends Component {
  #el_search_results = null;
  #last_query = null;
  #active_index = -1;

  onSetup() {
    Component.useEvents({
      search: {
        mode: 'id',
        events: {
          input: this.onInputModuleSearch,
          keydown: this.onKeyDownModuleSearch,
        },
      },
    });
    Component.useFetchData({
      modules: {
        endpoint: '/common/odoo/module/list',
        method: HTTP_METHOD.GET,
      },
    });
  }

  async onWillStart() {
    await super.onWillStart(...arguments);
    this.#el_search_results = this.queryId('results');
  }

  onStart() {
    super.onStart();
    this.#el_search_results.style.display = 'none';
  }

  onInputModuleSearch(ev) {
    const cur_active_item = this.#active_index;
    this.#active_index = -1;
    this.#updateActiveItem(cur_active_item);
    this.#last_query = ev.target.value.replaceAll(' ', '_');
    if (this.#last_query === '') {
      this.#fillResults();
    } else {
      this.#filterResults(this.#last_query).then(filter_info => {
        if (filter_info[0] === this.#last_query) {
          this.#fillResults(filter_info[1]);
        }
      });
    }
  }

  onKeyDownModuleSearch(ev) {
    if (ev.code === 'ArrowDown') {
      ev.preventDefault();
      this.#selectNextItem();
    } else if (ev.code === 'ArrowUp') {
      ev.preventDefault();
      this.#selectPrevItem();
    } else if (ev.code === 'Enter') {
      ev.preventDefault();
      if (this.#active_index !== -1) {
        const el_li = this.#el_search_results.children[this.#active_index];
        el_li.querySelector('.item').click();
      }
    }
  }

  #selectNextItem() {
    const len_results = this.#el_search_results.children.length;
    const max = Math.min(len_results, MAX_RESULTS);
    const cur_active_item = this.#active_index;
    this.#active_index += 1;
    if (this.#active_index === max) {
      this.#active_index = max - 1;
    }
    this.#updateActiveItem(cur_active_item);
  }

  #selectPrevItem() {
    const cur_active_item = this.#active_index;
    this.#active_index -= 1;
    if (this.#active_index === -1) {
      this.#active_index = 0;
    }
    this.#updateActiveItem(cur_active_item);
  }

  #updateActiveItem(last_active_index) {
    if (typeof last_active_index !== 'undefined' && last_active_index !== -1) {
      const el_li = this.#el_search_results.children[last_active_index];
      el_li.classList.remove('active');
    }
    if (this.#active_index !== -1) {
      const el_li = this.#el_search_results.children[this.#active_index];
      el_li.classList.add('active');
      el_li.scrollIntoView({
        block: 'center',
        inline: 'nearest',
      });
    }
  }

  async #filterResults(query) {
    const modules = this.getFetchData('modules');
    const filtered = modules.filter(item =>
      item.technical_name.includes(query.toLowerCase()),
    );
    return [query, filtered];
  }

  #fillResults(results) {
    if (typeof results === 'undefined') {
      this.#el_search_results.style.display = 'none';
      return;
    }

    this.#el_search_results.replaceChildren();
    const count_items = results.length;
    const results_to_render =
      count_items > MAX_RESULTS ? results.slice(0, MAX_RESULTS) : results;
    let result_items = results_to_render.map(module => {
      const item_container = document.createElement('li');
      const item = document.createElement('a');
      item.classList.add('item');
      item.href = `/module/${module.org_name}/${module.technical_name}`;
      item.innerHTML = `<div>${module.technical_name}</div><div class="info">${module.org_name}: ${module.versions.join(' - ')}<div>`;
      item_container.appendChild(item);
      return item_container;
    });
    if (results_to_render.length !== count_items) {
      result_items = result_items.slice(0, MAX_RESULTS);
      const item_container = document.createElement('li');
      item_container.style.textAlign = 'center';
      item_container.style.padding = '0.2em';
      item_container.style.color = 'lightyellow';
      item_container.style.backgroundColor = 'burlywood';
      item_container.textContent = `${count_items - MAX_RESULTS} hidden...`;
      result_items.push(item_container);
    }
    if (count_items === 0) {
      this.#el_search_results.style.display = 'none';
    } else {
      this.#el_search_results.append(...result_items);
      if (this.#active_index === -1) {
        this.#active_index = 0;
        this.#updateActiveItem();
      }
      this.#el_search_results.style.display = '';
    }
  }
}

registerComponent('module-search', ModuleSearch);
