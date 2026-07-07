// Copyright Alexandre D. Díaz
import {Component, registerComponent, HTTP_METHOD} from 'mirlo';
import '@scss/components/committer-search.scss';

const PAGE_SIZE = 50;
const SCROLL_THRESHOLD_PX = 100;

class CommitterSearch extends Component {
  #el_search_results = null;
  #last_query = null;
  #active_index = -1;
  #filtered_results = [];
  #rendered_count = 0;

  onSetup() {
    Component.useEvents({
      search: {
        mode: 'id',
        events: {
          input: this.onInputCommitterSearch,
          keydown: this.onKeyDownCommitterSearch,
        },
      },
      results: {
        mode: 'id',
        events: {
          scroll: this.onScrollResults,
        },
      },
    });
    Component.useFetchData({
      committers: {
        endpoint: '/common/odoo/committer/list',
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

  onInputCommitterSearch(ev) {
    const cur_active_item = this.#active_index;
    this.#active_index = -1;
    this.#updateActiveItem(cur_active_item);
    this.#last_query = ev.target.value;
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

  onKeyDownCommitterSearch(ev) {
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

  // ponytail: plain scroll listener, not IntersectionObserver like logs-viewer -
  // results are already fully fetched in memory, so there's no request to
  // prefetch, just an array slice to append.
  onScrollResults() {
    const el = this.#el_search_results;
    if (
      el.scrollTop + el.clientHeight >=
      el.scrollHeight - SCROLL_THRESHOLD_PX
    ) {
      this.#renderNextBatch();
    }
  }

  #selectNextItem() {
    if (
      this.#active_index + 1 >= this.#rendered_count &&
      this.#rendered_count < this.#filtered_results.length
    ) {
      this.#renderNextBatch();
    }
    const len_results = this.#el_search_results.children.length;
    const cur_active_item = this.#active_index;
    this.#active_index += 1;
    if (this.#active_index >= len_results) {
      this.#active_index = len_results - 1;
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
    const committers = this.getFetchData('committers');
    const lower_query = query.toLowerCase();
    const filtered = committers.filter(item =>
      item.name.toLowerCase().includes(lower_query),
    );
    return [query, filtered];
  }

  #fillResults(results) {
    this.#el_search_results.replaceChildren();
    this.#filtered_results = results ?? [];
    this.#rendered_count = 0;

    if (this.#filtered_results.length === 0) {
      this.#el_search_results.style.display = 'none';
      return;
    }

    this.#renderNextBatch();
    this.#el_search_results.style.display = '';
    if (this.#active_index === -1) {
      this.#active_index = 0;
      this.#updateActiveItem();
    }
  }

  #renderNextBatch() {
    if (this.#rendered_count >= this.#filtered_results.length) {
      return;
    }
    const next_batch = this.#filtered_results.slice(
      this.#rendered_count,
      this.#rendered_count + PAGE_SIZE,
    );
    this.#rendered_count += next_batch.length;
    this.#el_search_results.append(
      ...next_batch.map(committer => this.#createResultItem(committer)),
    );
  }

  #createResultItem(committer) {
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
