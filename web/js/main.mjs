// Copyright Alexandre D. Díaz
import * as bootstrap from 'bootstrap/dist/js/bootstrap.bundle.js';
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
      document.documentElement.dataset.bsTheme === 'light' ? '☀️' : '🌙';
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

if (document.readyState === 'loading') {
  document.addEventListener('DOMContentLoaded', () => {
    initModeSwitch();
    initThemeSwitch();
  });
} else {
  initModeSwitch();
  initThemeSwitch();
}
