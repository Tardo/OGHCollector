// Copyright Alexandre D. Díaz
import {Component, HTTP_METHOD} from 'mirlo';

const PAGE_SIZE = 50;
const SCROLL_THRESHOLD_PX = 100;
const DEBOUNCE_MS = 150;

// Shared base for the module/committer search dropdowns. Subclasses define:
//   get searchEndpoint() -> URL of the full record list
//   searchKey(record)    -> string the query is matched against
//   createResultItem(record) -> <li> element for one result
// and may override normalizeQuery(query).
export default class SearchDropdown extends Component {
  #el_search_results = null;
  #active_index = -1;
  #filtered_results = [];
  #rendered_count = 0;
  #debounce_timer = null;
  #search_index = null;
  #prev_query = null;
  #prev_matches = null;

  onSetup() {
    Component.useEvents(this.getEventDefs());
    Component.useFetchData({
      records: {
        endpoint: this.searchEndpoint,
        method: HTTP_METHOD.GET,
      },
    });
  }

  // Subclasses that add their own controls (e.g. field/version filters)
  // override this and spread `...super.getEventDefs()` in, instead of
  // clobbering the base search/results bindings with their own useEvents call.
  getEventDefs() {
    return {
      search: {
        mode: 'id',
        events: {
          input: this.onInputSearch,
          keydown: this.onKeyDownSearch,
        },
      },
      results: {
        mode: 'id',
        events: {
          scroll: this.onScrollResults,
        },
      },
    };
  }

  // Subclasses with extra filter controls (dropdowns, checkboxes...) override
  // this instead of searchKey/normalizeQuery to exclude records outright.
  recordMatchesFilters() {
    return true;
  }

  // Re-runs the current search text against the current filters/field. Call
  // after changing a filter control so results reflect it immediately.
  refreshResults() {
    this.#search_index = null;
    this.#prev_query = null;
    this.#prev_matches = null;
    this.#active_index = -1;
    const query = this.normalizeQuery(this.queryId('search').value);
    if (query === '') {
      this.#fillResults();
    } else {
      this.#fillResults(this.#filterResults(query));
    }
  }

  async onWillStart() {
    await super.onWillStart(...arguments);
    this.#el_search_results = this.queryId('results');
  }

  onStart() {
    super.onStart();
    this.#el_search_results.style.display = 'none';
  }

  normalizeQuery(query) {
    return query.toLowerCase();
  }

  onInputSearch(ev) {
    clearTimeout(this.#debounce_timer);
    const cur_active_item = this.#active_index;
    this.#active_index = -1;
    this.#updateActiveItem(cur_active_item);
    const query = this.normalizeQuery(ev.target.value);
    if (query === '') {
      this.#prev_query = null;
      this.#prev_matches = null;
      this.#fillResults();
    } else {
      this.#debounce_timer = setTimeout(() => {
        this.#fillResults(this.#filterResults(query));
      }, DEBOUNCE_MS);
    }
  }

  onKeyDownSearch(ev) {
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
      // 'nearest' instead of 'center': it's a no-op when the item is already
      // visible (every keystroke re-highlights item 0), never scrolls the
      // page, and avoids Firefox's animated programmatic scrolls stacking up.
      el_li.scrollIntoView({
        block: 'nearest',
        inline: 'nearest',
      });
    }
  }

  #filterResults(query) {
    if (this.#search_index === null) {
      // Lowercase every key once instead of on every keystroke.
      this.#search_index = this.getFetchData('records')
        .filter(record => this.recordMatchesFilters(record))
        .map(record => ({
          key: this.searchKey(record).toLowerCase(),
          record,
        }));
    }
    // Typing usually extends the previous query: narrowing the previous
    // matches instead of rescanning the full index keeps each keystroke
    // proportional to the current result set, not the whole list.
    const base =
      this.#prev_matches !== null && query.startsWith(this.#prev_query)
        ? this.#prev_matches
        : this.#search_index;
    const matches = base.filter(entry => entry.key.includes(query));
    this.#prev_query = query;
    this.#prev_matches = matches;
    return matches.map(entry => entry.record);
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
      ...next_batch.map(record => this.createResultItem(record)),
    );
  }
}
