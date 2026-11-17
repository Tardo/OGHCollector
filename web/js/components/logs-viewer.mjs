// Copyright 2026 Alexandre D. Díaz
import {Component, registerComponent, getService} from 'mirlo';
import '@scss/components/logs-viewer.scss';
import {t} from '@app/utils/i18n';

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

function escapeHtml(text) {
  return String(text).replace(
    /[&<>"']/g,
    c =>
      ({'&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'})[
        c
      ],
  );
}

class LogsViewer extends Component {
  #el_date_from = null;
  #el_date_to = null;
  #el_status = null;
  #el_sentinel = null;
  #observer = null;
  #oldest_id = null;
  #done = false;
  #loading = false;

  // Last header emitted per level, so paginated fetches keep appending
  // under the open day instead of repeating its headers.
  #last_year = null;
  #last_month = null;
  #last_day = null;

  onSetup() {
    // Flat list: header entries (year/month/day) and log rows share one
    // template; `cls` picks the look, the CSS hides the unused cells.
    Component.useStateBinds({
      entries: {
        id: 'rows',
        each: {
          template: 'tpl-entry',
          key: 'key',
          binds: {
            cls: {attribute: 'class'},
            time: {selector: '.log-time'},
            type: {selector: '.log-type-badge'},
            html: {selector: '.log-message', attribute: 'html'},
          },
        },
      },
    });
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
    this.#el_date_from = this.queryId('date_from');
    this.#el_date_to = this.queryId('date_to');
    this.#el_status = this.queryId('status');
    this.#el_sentinel = this.queryId('sentinel');
  }

  onStart() {
    super.onStart();
    this.mirlo.state.entries = [];
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
    this.mirlo.state.entries = [];
    this.#oldest_id = null;
    this.#done = false;
    this.#last_year = null;
    this.#last_month = null;
    this.#last_day = null;
    this.#loadNextPage();
  }

  async #loadNextPage() {
    if (this.#loading || this.#done) {
      return;
    }
    this.#loading = true;
    this.#el_status.textContent = t('Loading...');
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
        ? this.mirlo.state.entries.length === 0
          ? t('No log entries found.')
          : t('End of log.')
        : '';
    } catch {
      this.#el_status.textContent = t('Failed to load logs.');
    } finally {
      this.#loading = false;
    }
  }

  #header(key, cls, label) {
    return {key, cls, time: '', type: '', html: escapeHtml(label)};
  }

  #appendRows(events) {
    const entries = [];
    for (const event of events) {
      const [date_part, time_part] = event.date.split(' ');
      const year = date_part.slice(0, 4);
      const month = date_part.slice(0, 7);
      if (year !== this.#last_year) {
        entries.push(this.#header(`y:${year}`, 'year-header', year));
        this.#last_year = year;
        this.#last_month = null;
      }
      if (month !== this.#last_month) {
        entries.push(
          this.#header(
            `m:${month}`,
            'month-header',
            MONTH_FORMATTER.format(new Date(`${month}-01`)),
          ),
        );
        this.#last_month = month;
        this.#last_day = null;
      }
      if (date_part !== this.#last_day) {
        entries.push(
          this.#header(
            `d:${date_part}`,
            'day-header',
            DAY_FORMATTER.format(new Date(date_part)),
          ),
        );
        this.#last_day = date_part;
      }
      entries.push({
        key: `e:${event.id}`,
        cls: `log-row log-severity-${event.severity}`,
        time: time_part,
        type: event.event_type_name,
        // ponytail: is_html rows are trusted, same boundary as the old
        // `| safe` Jinja rendering; only pre-rewrite rows ever set it.
        html: event.is_html ? event.message : escapeHtml(event.message),
      });
    }
    this.mirlo.state.entries.push(...entries);
  }
}
registerComponent('logs-viewer', LogsViewer);
