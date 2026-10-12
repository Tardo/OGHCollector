// Copyright Alexandre D. Díaz
import * as bootstrap from 'bootstrap/dist/js/bootstrap.bundle.js';
import '@app/components/module-search';
import '@app/components/committer-search';
import '@scss/main.scss';

window.bootstrap = bootstrap;

// Restarts a CSS animation bound to `className` on `el`. remove() then
// add() back-to-back isn't enough on repeat triggers - without a style
// flush in between, the browser can coalesce both mutations into one
// recalc and never see the class as "removed", so the animation doesn't
// restart. Reading offsetWidth forces that flush.
function restartAnimation(el, className) {
  el.classList.remove(className);
  const _ = el.offsetWidth; // read forces the reflow; value itself unused
  el.classList.add(className);
}

// User/Developer mode switch (data-mode is set pre-paint in base_layout.html)
function initModeSwitch() {
  const dev_switch = document.getElementById('dev-mode-switch');
  if (!dev_switch) {
    return;
  }
  dev_switch.checked = document.documentElement.dataset.mode === 'dev';
  dev_switch.addEventListener('change', () => {
    const mode = dev_switch.checked ? 'dev' : 'user';
    document.documentElement.dataset.mode = mode;
    localStorage.setItem('ommd_mode', mode);
  });
}

// Light/Dark theme toggle (data-bs-theme is set pre-paint in base_layout.html)
function initThemeSwitch() {
  const theme_toggle = document.getElementById('theme-toggle');
  if (!theme_toggle) {
    return;
  }
  const icon = theme_toggle.querySelector('.theme-icon');
  const syncIcon = () => {
    icon.textContent =
      document.documentElement.dataset.bsTheme === 'light'
        ? 'light_mode'
        : 'dark_mode';
  };
  syncIcon();
  theme_toggle.addEventListener('click', () => {
    const theme =
      document.documentElement.dataset.bsTheme === 'light' ? 'dark' : 'light';
    document.documentElement.dataset.bsTheme = theme;
    localStorage.setItem('ommd_theme', theme);
    syncIcon();
    restartAnimation(icon, 'spin');
  });
}

// Workspace selector: listings are filtered server-side by this cookie.
function initWorkspaceSelect() {
  const select = document.getElementById('workspace-select');
  if (!select) {
    return;
  }
  select.addEventListener('change', () => {
    const value = encodeURIComponent(select.value);
    const max_age = select.value ? 31536000 : 0;
    document.cookie = `workspace=${value}; path=/; max-age=${max_age}; SameSite=Lax`;
    location.reload();
  });
}

// Desktop sidebar: expanded <-> icon rail (data-sidebar is set pre-paint in
// base_layout.html). Below lg the sidebar is a Bootstrap offcanvas instead.
function initSidebarToggle() {
  const toggle = document.getElementById('sidebar-toggle');
  if (!toggle) {
    return;
  }
  const sync = () => {
    const collapsed = document.documentElement.dataset.sidebar === 'collapsed';
    toggle.querySelector('.material-icons').textContent = collapsed
      ? 'menu'
      : 'menu_open';
    toggle.title = collapsed ? 'Expand sidebar' : 'Collapse sidebar';
    toggle.setAttribute('aria-label', toggle.title);
  };
  sync();
  toggle.addEventListener('click', () => {
    const state =
      document.documentElement.dataset.sidebar === 'collapsed'
        ? 'expanded'
        : 'collapsed';
    document.documentElement.dataset.sidebar = state;
    localStorage.setItem('ommd_sidebar', state);
    sync();
  });
}

// Keystrokes inside a field (or a mirlo component, whose closed shadow DOM
// retargets the event to its host) belong to that field, not to the palette.
function isTypingTarget(el) {
  return (
    el.isContentEditable ||
    el.tagName.includes('-') ||
    el.closest('input, textarea, select') !== null
  );
}

// Global search palette. The search components are only created on first
// open: each one fetches its full record list as soon as it's mounted.
function initSearchPalette() {
  const palette = document.getElementById('search-palette');
  if (!palette) {
    return;
  }
  const modal = bootstrap.Modal.getOrCreateInstance(palette);
  const focusSearch = () => {
    const pane = palette.querySelector('.tab-pane.active');
    let search = pane.firstElementChild;
    if (!search) {
      search = document.createElement(pane.dataset.searchTag);
      search.classList.add('d-block');
      pane.appendChild(search);
    }
    search.query('#search').focus();
  };
  // The modal has no `fade`, so show() is synchronous and focus moves into
  // the input before the key that opened it is inserted (type-to-search).
  const open = () => {
    modal.show();
    focusSearch();
  };
  palette.addEventListener('shown.bs.tab', focusSearch);
  for (const trigger of document.querySelectorAll('[data-search-open]')) {
    trigger.addEventListener('click', open);
  }
  document.addEventListener('keydown', ev => {
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'k') {
      ev.preventDefault();
      open();
      return;
    }
    if (
      ev.ctrlKey ||
      ev.metaKey ||
      ev.altKey ||
      palette.classList.contains('show') ||
      isTypingTarget(ev.target)
    ) {
      return;
    }
    if (ev.key === '/') {
      ev.preventDefault();
      open();
    } else if (ev.key.length === 1 && ev.key.trim() !== '') {
      // Pages with their own search (/modules, /committers) take the
      // keystroke there instead of opening a second search box on top.
      const inline = document.querySelector(
        '#app-content :is(mirlo-module-search, mirlo-committer-search)',
      );
      if (inline) {
        inline.query('#search').focus();
      } else {
        open();
      }
    }
  });
}

function init() {
  initModeSwitch();
  initThemeSwitch();
  initWorkspaceSelect();
  initSidebarToggle();
  initSearchPalette();
}

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', init);
} else {
  init();
}
