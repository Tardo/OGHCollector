// Copyright 2026 Alexandre D. Díaz

// Fetches `url` and swaps it in as `pane`'s content. `onSuccess` only fires
// on a real 2xx response, so a caller tracking "still needs loading" state
// (see initLazyTabPanes below) can retry later instead of getting stuck
// after a transient failure.
export function fetchIntoPane(pane, url, onSuccess) {
  fetch(url)
    .then(res => (res.ok ? res.text() : Promise.reject(res.status)))
    .then(html => {
      pane.innerHTML = html;
      onSuccess?.();
    })
    .catch(() => {
      pane.innerHTML = '<p class="text-danger">Failed to load this tab.</p>';
    });
}

// Tab panes marked `data-lazy-tab-url` are rendered empty by the server;
// fetch their content on first activation instead of shipping every tab's
// data on initial page load (see e.g. pages/module.html, pages/modules.html,
// pages/osv.html).
export function initLazyTabPanes(tabListId) {
  document.getElementById(tabListId)?.addEventListener('shown.bs.tab', ev => {
    const targetId = ev.target.getAttribute('data-bs-target')?.slice(1);
    const pane = targetId && document.getElementById(targetId);
    const url = pane?.dataset.lazyTabUrl;
    if (!url) {
      return;
    }
    fetchIntoPane(pane, url, () => delete pane.dataset.lazyTabUrl);
  });
}
