// Copyright 2026 Alexandre D. Díaz
import {Component} from 'mirlo';
import {t} from '@app/utils/i18n';

// Fetches `url` and swaps it in as `pane`'s content. `onSuccess` only fires
// on a real 2xx response, so a caller tracking "still needs loading" state
// can retry later instead of getting stuck after a transient failure. The
// pane then dispatches a bubbling `lazy-tab-loaded`, for page code that
// post-processes tab content (osv re-applies its filter).
export function fetchIntoPane(pane, url, onSuccess) {
  fetch(url)
    .then(res => (res.ok ? res.text() : Promise.reject(res.status)))
    .then(html => {
      pane.innerHTML = html;
      onSuccess?.();
      pane.dispatchEvent(new Event('lazy-tab-loaded', {bubbles: true}));
    })
    .catch(() => {
      const msg = document.createElement('p');
      msg.className = 'text-danger';
      msg.textContent = t('Failed to load this tab.');
      pane.replaceChildren(msg);
    });
}

// Base of the pages with Bootstrap tabs: panes marked `data-lazy-tab-url` are
// rendered empty by the server, and fetched on first activation instead of
// shipping every tab's data on initial page load (pages/module.html,
// modules.html, osv.html). Light DOM, no template: it wraps the server's
// markup (outside `.tab-content`, whose `> .tab-pane` rule needs its children).
// Subclasses add their own events through getEventDefs().
export default class LazyTabs extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents(this.getEventDefs());
  }

  // '' = the host: `shown.bs.tab` bubbles up from the tab button.
  getEventDefs() {
    return {'': {events: {'shown.bs.tab': this.onShownTab}}};
  }

  onShownTab(ev) {
    const targetId = ev.target.getAttribute('data-bs-target')?.slice(1);
    const pane = targetId && document.getElementById(targetId);
    const url = pane?.dataset.lazyTabUrl;
    if (!url) {
      return;
    }
    fetchIntoPane(pane, url, () => delete pane.dataset.lazyTabUrl);
  }
}
