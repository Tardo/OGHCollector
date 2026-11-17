// Copyright 2026 Alexandre D. Díaz
// App shell controls (minimal_layout.html). Each one wraps markup the server
// already rendered (no template), in light DOM so Bootstrap applies; the host
// is `display: contents` (custom.scss) so the layout doesn't see it.
import * as bootstrap from 'bootstrap/dist/js/bootstrap.bundle.js';
import {Component, registerComponent} from 'mirlo';
import {t} from '@app/utils/i18n';

function setCookie(name, value, max_age) {
  document.cookie = `${name}=${value}; path=/; max-age=${max_age}; SameSite=Lax`;
}

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

// User/Developer mode (data-mode is set pre-paint in base_layout.html)
class ModeSwitch extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      'dev-mode-switch': {mode: 'id', events: {change: this.onChange}},
    });
  }

  onStart() {
    super.onStart();
    this.queryId('dev-mode-switch').checked =
      document.documentElement.dataset.mode === 'dev';
  }

  onChange(ev) {
    const mode = ev.target.checked ? 'dev' : 'user';
    document.documentElement.dataset.mode = mode;
    localStorage.setItem('ommd_mode', mode);
  }
}

// Auto/Light/Dark (data-bs-theme is set pre-paint in base_layout.html;
// auto = no saved choice, resolved by the time of day there)
const THEME_ICONS = {
  auto: 'brightness_auto',
  light: 'light_mode',
  dark: 'dark_mode',
};
const THEME_NEXT = {auto: 'light', light: 'dark', dark: 'auto'};

class ThemeToggle extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      'theme-toggle': {mode: 'id', events: {click: this.onClick}},
    });
  }

  onStart() {
    super.onStart();
    this.#syncIcon();
  }

  get #choice() {
    return localStorage.getItem('ommd_theme') || 'auto';
  }

  #syncIcon() {
    const btn = this.queryId('theme-toggle');
    btn.querySelector('.theme-icon').textContent = THEME_ICONS[this.#choice];
    const [theme, change, ...names] = btn.dataset.i18nTheme.split('|');
    const label = {auto: names[0], light: names[1], dark: names[2]}[
      this.#choice
    ];
    btn.title = `${theme}: ${label} (${change})`;
  }

  onClick() {
    const next = THEME_NEXT[this.#choice];
    if (next === 'auto') {
      localStorage.removeItem('ommd_theme');
    } else {
      localStorage.setItem('ommd_theme', next);
    }
    document.documentElement.dataset.bsTheme =
      next === 'auto' ? window.ommdAutoTheme() : next;
    this.#syncIcon();
    restartAnimation(this.query('.theme-icon'), 'spin');
  }
}

// Language dropdown: auto (no cookie: the server uses the browser's language)
// or a fixed one. Pages are translated server-side, so a change reloads.
class LangSelect extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      'lang-select': {mode: 'id', events: {click: this.onClick}},
    });
  }

  onStart() {
    super.onStart();
    const root = this.queryId('lang-select');
    const match = document.cookie.match(/(?:^|; )lang=(en|es|gl)/);
    const choice = match ? match[1] : 'auto';
    root.querySelector('.lang-code').textContent =
      choice === 'auto' ? root.dataset.lang : choice;
    root
      .querySelector(`[data-lang-choice="${choice}"]`)
      .classList.add('active');
  }

  onClick(ev) {
    const item = ev.target.closest('[data-lang-choice]');
    if (!item) {
      return;
    }
    const next = item.dataset.langChoice;
    setCookie(
      'lang',
      next === 'auto' ? '' : next,
      next === 'auto' ? 0 : 31536000,
    );
    location.reload();
  }
}

// Workspace selector: listings are filtered server-side by this cookie.
class WorkspaceSelect extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      'workspace-select': {mode: 'id', events: {change: this.onChange}},
    });
  }

  onChange(ev) {
    const value = ev.target.value;
    setCookie('workspace', encodeURIComponent(value), value ? 31536000 : 0);
    location.reload();
  }
}

// Desktop sidebar: expanded <-> icon rail (data-sidebar is set pre-paint in
// base_layout.html). Below lg the sidebar is a Bootstrap offcanvas instead.
class SidebarToggle extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      'sidebar-toggle': {mode: 'id', events: {click: this.onClick}},
    });
  }

  onStart() {
    super.onStart();
    this.#sync();
  }

  #sync() {
    const toggle = this.queryId('sidebar-toggle');
    const collapsed = document.documentElement.dataset.sidebar === 'collapsed';
    toggle.querySelector('.material-icons').textContent = collapsed
      ? 'menu'
      : 'menu_open';
    toggle.title = collapsed ? t('Expand sidebar') : t('Collapse sidebar');
    toggle.setAttribute('aria-label', toggle.title);
  }

  onClick() {
    const state =
      document.documentElement.dataset.sidebar === 'collapsed'
        ? 'expanded'
        : 'collapsed';
    document.documentElement.dataset.sidebar = state;
    localStorage.setItem('ommd_sidebar', state);
    this.#sync();
  }
}

// Keystrokes inside a field (or a mirlo component whose shadow DOM retargets
// the event to its host) belong to that field, not to the palette.
function isTypingTarget(el) {
  return (
    el.isContentEditable ||
    el.tagName.includes('-') ||
    el.closest('input, textarea, select') !== null
  );
}

// Global search palette (Ctrl/Cmd+K, `/` or just typing). The search
// components are only created on first open: each one fetches its full record
// list as soon as it's mounted. Wraps the modal; the `[data-search-open]`
// triggers live elsewhere in the page.
class SearchPalette extends Component {
  #modal = null;
  #palette = null;
  #onKeyDown = ev => this.#handleKey(ev);

  onSetup() {
    Component.disableShadow();
    Component.useEvents({
      'search-palette': {
        mode: 'id',
        events: {'shown.bs.tab': this.focusSearch},
      },
    });
  }

  onStart() {
    super.onStart();
    this.#palette = this.queryId('search-palette');
    this.#modal = bootstrap.Modal.getOrCreateInstance(this.#palette);
    for (const trigger of document.querySelectorAll('[data-search-open]')) {
      trigger.addEventListener('click', () => this.open());
    }
    document.addEventListener('keydown', this.#onKeyDown);
  }

  onRemove() {
    document.removeEventListener('keydown', this.#onKeyDown);
  }

  focusSearch() {
    const pane = this.#palette.querySelector('.tab-pane.active');
    let search = pane.firstElementChild;
    if (!search) {
      search = document.createElement(pane.dataset.searchTag);
      search.classList.add('d-block');
      pane.appendChild(search);
    }
    search.query('#search').focus();
  }

  // The modal has no `fade`, so show() is synchronous and focus moves into
  // the input before the key that opened it is inserted (type-to-search).
  open() {
    this.#modal.show();
    this.focusSearch();
  }

  #handleKey(ev) {
    if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === 'k') {
      ev.preventDefault();
      this.open();
      return;
    }
    if (
      ev.ctrlKey ||
      ev.metaKey ||
      ev.altKey ||
      this.#palette.classList.contains('show') ||
      isTypingTarget(ev.target)
    ) {
      return;
    }
    if (ev.key === '/') {
      ev.preventDefault();
      this.open();
    } else if (ev.key.length === 1 && ev.key.trim() !== '') {
      // Pages with their own search (/modules, /committers) take the
      // keystroke there instead of opening a second search box on top.
      const inline = document.querySelector(
        '#app-content :is(mirlo-module-search, mirlo-committer-search)',
      );
      if (inline) {
        inline.query('#search').focus();
      } else {
        this.open();
      }
    }
  }
}

registerComponent('mode-switch', ModeSwitch);
registerComponent('theme-toggle', ThemeToggle);
registerComponent('lang-select', LangSelect);
registerComponent('workspace-select', WorkspaceSelect);
registerComponent('sidebar-toggle', SidebarToggle);
registerComponent('search-palette', SearchPalette);
