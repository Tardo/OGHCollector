// Copyright 2026 Alexandre D. Díaz
import {Component, registerComponent, getService} from 'mirlo';
import '@scss/components/logs-viewer.scss';

const PAGE_SIZE = 200;
const MONTH_FORMATTER = new Intl.DateTimeFormat(undefined, {
  month: 'long',
  timeZone: 'UTC',
});
const DAY_FORMATTER = new Intl.DateTimeFormat(undefined, {
  weekday: 'long',
  day: 'numeric',
  timeZone: 'UTC',
});

function formatUTCDate(year, month_index, day) {
  return new Date(Date.UTC(year, month_index, day)).toISOString().slice(0, 10);
}

class LogsViewer extends Component {
  #el_rows = null;
  #el_date_from = null;
  #el_date_to = null;
  #el_status = null;
  #el_sentinel = null;
  #observer = null;
  #oldest_id = null;
  #done = false;
  #loading = false;

  // Tracks the currently open group so paginated fetches can keep appending
  // to it instead of starting a new group when a page happens to split a day.
  #current_year_key = null;
  #current_month_key = null;
  #current_day_key = null;
  #el_year_group = null;
  #el_month_group = null;
  #el_day_group = null;

  onSetup() {
    Component.useEvents({
      date_from: {
        mode: 'id',
        events: {change: this.onChangeFilter},
      },
      date_to: {
        mode: 'id',
        events: {change: this.onChangeFilter},
      },
      quick_today: {
        mode: 'id',
        events: {click: this.onClickToday},
      },
      quick_month: {
        mode: 'id',
        events: {click: this.onClickThisMonth},
      },
      quick_year: {
        mode: 'id',
        events: {click: this.onClickThisYear},
      },
      clear_filter: {
        mode: 'id',
        events: {click: this.onClickClear},
      },
    });
  }

  async onWillStart() {
    await super.onWillStart(...arguments);
    this.#el_rows = this.queryId('rows');
    this.#el_date_from = this.queryId('date_from');
    this.#el_date_to = this.queryId('date_to');
    this.#el_status = this.queryId('status');
    this.#el_sentinel = this.queryId('sentinel');
  }

  onStart() {
    super.onStart();
    this.#observer = new IntersectionObserver(
      entries => {
        if (entries[0].isIntersecting) {
          this.#loadNextPage();
        }
      },
      {rootMargin: '200px'},
    );
    this.#observer.observe(this.#el_sentinel);
    this.#loadNextPage();
  }

  onRemove() {
    this.#observer?.disconnect();
  }

  onChangeFilter() {
    this.#resetAndReload();
  }

  onClickToday() {
    const today = new Date().toISOString().slice(0, 10);
    this.#setRange(today, today);
  }

  onClickThisMonth() {
    const now = new Date();
    const year = now.getUTCFullYear();
    const month = now.getUTCMonth();
    this.#setRange(
      formatUTCDate(year, month, 1),
      formatUTCDate(year, month + 1, 0),
    );
  }

  onClickThisYear() {
    const year = new Date().getUTCFullYear();
    this.#setRange(`${year}-01-01`, `${year}-12-31`);
  }

  onClickClear() {
    this.#setRange('', '');
  }

  #setRange(from, to) {
    this.#el_date_from.value = from;
    this.#el_date_to.value = to;
    this.#resetAndReload();
  }

  #resetAndReload() {
    this.#el_rows.replaceChildren();
    this.#oldest_id = null;
    this.#done = false;
    this.#current_year_key = null;
    this.#current_month_key = null;
    this.#current_day_key = null;
    this.#el_year_group = null;
    this.#el_month_group = null;
    this.#el_day_group = null;
    this.#loadNextPage();
  }

  async #loadNextPage() {
    if (this.#loading || this.#done) {
      return;
    }
    this.#loading = true;
    this.#el_status.textContent = 'Loading...';
    try {
      const params = new URLSearchParams({limit: String(PAGE_SIZE)});
      if (this.#oldest_id !== null) {
        params.set('before_id', String(this.#oldest_id));
      }
      if (this.#el_date_from.value) {
        params.set('date_from', this.#el_date_from.value);
      }
      if (this.#el_date_to.value) {
        params.set('date_to', this.#el_date_to.value);
      }
      const events = await getService('requests').getJSON(
        `/logs/data?${params}`,
      );
      if (events.length > 0) {
        this.#oldest_id = events[events.length - 1].id;
        this.#appendRows(events);
      }
      this.#done = events.length < PAGE_SIZE;
      this.#el_status.textContent = this.#done
        ? this.#el_rows.children.length === 0
          ? 'No log entries found.'
          : 'End of log.'
        : '';
    } catch {
      this.#el_status.textContent = 'Failed to load logs.';
    } finally {
      this.#loading = false;
    }
  }

  #createHeader(text, class_name) {
    const el = document.createElement('div');
    el.className = class_name;
    el.textContent = text;
    return el;
  }

  #openYearGroup(year_key) {
    const group = document.createElement('div');
    group.className = 'year-group';
    group.appendChild(this.#createHeader(year_key, 'year-header'));
    this.#el_rows.appendChild(group);
    this.#current_year_key = year_key;
    this.#current_month_key = null;
    return group;
  }

  #openMonthGroup(year_group, date_part) {
    const group = document.createElement('div');
    group.className = 'month-group';
    const label = MONTH_FORMATTER.format(
      new Date(`${date_part.slice(0, 7)}-01`),
    );
    group.appendChild(this.#createHeader(label, 'month-header'));
    year_group.appendChild(group);
    this.#current_month_key = date_part.slice(0, 7);
    this.#current_day_key = null;
    return group;
  }

  #openDayGroup(month_group, date_part) {
    const group = document.createElement('div');
    group.className = 'day-group';
    const label = DAY_FORMATTER.format(new Date(date_part));
    group.appendChild(this.#createHeader(label, 'day-header'));
    month_group.appendChild(group);
    this.#current_day_key = date_part;
    return group;
  }

  #createRow(event, time_part) {
    const row = document.createElement('div');
    row.className = `log-row log-severity-${event.severity}`;

    const time_el = document.createElement('span');
    time_el.className = 'log-time';
    time_el.textContent = time_part;

    const type_el = document.createElement('span');
    type_el.className = 'log-type-badge';
    type_el.textContent = event.event_type_name;

    const msg_el = document.createElement('span');
    msg_el.className = 'log-message';
    if (event.is_html) {
      // ponytail: trusted data, same trust boundary as the old `| safe`
      // Jinja rendering it replaces - only pre-rewrite rows ever set is_html.
      msg_el.innerHTML = event.message;
    } else {
      msg_el.textContent = event.message;
    }

    row.append(time_el, type_el, msg_el);
    return row;
  }

  #appendRows(events) {
    for (const event of events) {
      const [date_part, time_part] = event.date.split(' ');
      const year_key = date_part.slice(0, 4);
      const month_key = date_part.slice(0, 7);

      if (year_key !== this.#current_year_key) {
        this.#el_year_group = this.#openYearGroup(year_key);
      }
      if (month_key !== this.#current_month_key) {
        this.#el_month_group = this.#openMonthGroup(
          this.#el_year_group,
          date_part,
        );
      }
      if (date_part !== this.#current_day_key) {
        this.#el_day_group = this.#openDayGroup(
          this.#el_month_group,
          date_part,
        );
      }
      this.#el_day_group.appendChild(this.#createRow(event, time_part));
    }
  }
}

registerComponent('logs-viewer', LogsViewer);
