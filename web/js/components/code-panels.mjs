// Copyright 2025 Alexandre D. Díaz
// "Copy" button ([data-copy]) of a .code-panel: copies the panel's <pre>.
// Wraps a page's content (light DOM, no template) and delegates the click.
// Outside a secure context (no clipboard API) it selects the snippet for
// Ctrl+C instead, like the workflow editor.
import {Component, registerComponent} from 'mirlo';
import {t} from '@app/utils/i18n';

class CodePanels extends Component {
  onSetup() {
    Component.disableShadow();
    Component.useEvents({'': {events: {click: this.onClick}}});
  }

  async onClick(ev) {
    const btn = ev.target.closest('[data-copy]');
    if (!btn) {
      return;
    }
    const code = btn.closest('.code-panel').querySelector('pre');
    try {
      await navigator.clipboard.writeText(code.textContent);
      btn.textContent = t('Copied!');
    } catch {
      window.getSelection().selectAllChildren(code);
      btn.textContent = t('Press Ctrl+C');
    }
    setTimeout(() => {
      btn.textContent = t('Copy');
    }, 1500);
  }
}

registerComponent('code-panels', CodePanels);
