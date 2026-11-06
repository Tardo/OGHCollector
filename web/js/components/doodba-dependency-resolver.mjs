// Copyright 2025 Alexandre D. Díaz
import {Component, registerComponent, getService, HTTP_METHOD} from 'mirlo';
import * as yaml from 'js-yaml';
import JSZip from 'jszip';
import {
  setDragPanelProcessing,
  showDragPanelError,
  isYamlFile,
} from '@app/utils/drag-panel-processing';
import {renderLicenseReview} from '@app/utils/license-review';
import '@scss/components/doodba-dependency-resolver.scss';
import {t} from '@app/utils/i18n';

// UnresolvedModule statuses meaning the module is gone at that version.
const DROPPED = new Set(['renamed', 'merged', 'lost', 'later']);

class DoodbaDependencyResolver extends Component {
  #el_search_select_ver = null;
  #el_drag_panel = null;
  #el_result = null;
  #el_result_odoo = null;
  #el_result_pip = null;
  #el_result_bin = null;
  #el_license = null;
  #el_unresolved = null;
  #el_save = null;

  onSetup() {
    Component.useEvents({
      doodba_dep_resolver_drag_panel: {
        mode: 'id',
        events: {
          dragenter: this.onDragEnter,
          dragover: this.onDragOver,
          dragleave: this.onDragLeave,
          drop: this.onDrop,
          click: this.onClick,
        },
      },
      doodba_dep_resolver_save: {
        mode: 'id',
        events: {
          click: this.onClickSave,
        },
      },
    });
    Component.useFetchData({
      odoo_versions: {
        endpoint: '/common/odoo/versions',
        method: HTTP_METHOD.GET,
      },
    });
  }

  async onWillStart() {
    await super.onWillStart(...arguments);
    this.#el_search_select_ver = this.queryId(
      'doodba_dep_resolver_search_select_ver',
    );
    this.#el_drag_panel = this.queryId('doodba_dep_resolver_drag_panel');
    this.#el_result = this.queryId('doodba_dep_resolver_result');
    this.#el_result_odoo = this.queryId('doodba_dep_resolver_result_odoo');
    this.#el_result_pip = this.queryId('doodba_dep_resolver_result_pip');
    this.#el_result_bin = this.queryId('doodba_dep_resolver_result_bin');
    this.#el_license = this.queryId('doodba_dep_resolver_license');
    this.#el_unresolved = this.queryId('doodba_dep_resolver_unresolved');
    this.#el_save = this.queryId('doodba_dep_resolver_save');
  }

  onStart() {
    super.onStart();
    this.#fillOdooVersionsSearchOptions();
  }

  onDragEnter(ev) {
    ev.preventDefault();
    this.#el_drag_panel.classList.add('drag-over');
  }

  onDragOver(ev) {
    ev.preventDefault();
  }

  onDragLeave(ev) {
    ev.preventDefault();
    this.#el_drag_panel.classList.remove('drag-over');
  }

  async onClickSave() {
    const zip = new JSZip();
    zip.file('addons.yaml', this.#el_result_odoo.value);
    zip.file('pip.txt', this.#el_result_pip.value);
    zip.file('apt.txt', this.#el_result_bin.value);
    const zip_bin = await zip.generateAsync({type: 'blob'});
    const elem = window.document.createElement('a');
    const objURL = window.URL.createObjectURL(zip_bin);
    elem.href = objURL;
    elem.download = 'doodba_bundle.zip';
    document.body?.appendChild(elem);
    elem.click();
    document.body?.removeChild(elem);
    URL.revokeObjectURL(objURL);
  }

  #showResults(yaml_data, pip_data, bin_data) {
    this.#el_result_odoo.value = yaml_data;
    this.#el_result_pip.value = pip_data;
    this.#el_result_bin.value = bin_data;
    this.#el_drag_panel.style.display = 'none';
    this.#el_result.style.display = '';
    this.#el_save.style.display = '';
  }

  // Resolved modules (and their dependencies) go under the repo the server
  // found them in. Anything else from the input stays under its own key as
  // written (wildcards, ENV, open PRs, repos we don't collect), except the
  // modules the server says are gone at this version: those are dropped and
  // noted at the top.
  #makeYaml(data, yaml_data, dropped) {
    const resolved = new Set(Object.values(data).flat());
    for (const [repo_name, mod_names] of Object.entries(yaml_data)) {
      if (!Array.isArray(mod_names)) {
        // An emptied repo key is null: never over what was resolved there.
        if (mod_names) {
          data[repo_name] ??= mod_names;
        }
        continue;
      }
      for (const mod_name of mod_names) {
        if (!resolved.has(mod_name) && !dropped.has(mod_name)) {
          (data[repo_name] ??= []).push(mod_name);
        }
      }
    }

    const sortedGroupedData = Object.keys(data)
      .sort()
      .reduce((acc, key) => {
        acc[key] = data[key];
        return acc;
      }, {});
    const notes = [...dropped.values()].map(note => `# removed: ${note}\n`);
    return notes.join('') + yaml.dump(sortedGroupedData, {indent: 2});
  }

  // Why a requested module has no row at `odoo_ver` (see the server's
  // UnresolvedModule).
  #unresolvedReason(mod, odoo_ver) {
    const where = mod.repository ? ` (${mod.repository})` : '';
    switch (mod.status) {
      case 'renamed':
        return `renamed to ${mod.into} in ${mod.odoo_version}`;
      case 'merged':
        return `merged into ${mod.into} in ${mod.odoo_version}`;
      case 'lost':
        return `not in ${odoo_ver}, last seen in ${mod.odoo_version}${where}`;
      case 'later':
        return `not in ${odoo_ver}, newest in ${mod.odoo_version}${where}`;
      case 'pending':
        return `not merged in ${odoo_ver} yet, only an open PR/MR${where}: kept, merge it in repos.yaml`;
      case 'not_collected':
        return `its repository${where} isn't collected for ${odoo_ver}: kept as written`;
      default:
        return 'not found in any collected repository (private repo, fork or typo?): kept as written';
    }
  }

  // Plain DOM in the closed shadow root, reusing the license review panel's
  // look (web/scss/components/_license-review.scss).
  #renderUnresolved(unresolved, dropped, odoo_ver) {
    this.#el_unresolved.textContent = '';
    if (!unresolved.length) {
      return;
    }
    const section = document.createElement('section');
    section.className = 'license-review';
    section.dataset.worst = dropped.size ? 'warning' : 'info';
    const head = document.createElement('div');
    head.className = 'license-review-head';
    const icon = document.createElement('span');
    icon.className = 'material-icons';
    icon.setAttribute('aria-hidden', 'true');
    icon.textContent = 'report';
    const title = document.createElement('strong');
    title.textContent = t(
      '{0} module(s) not found in {1}',
      unresolved.length,
      odoo_ver,
    );
    head.append(icon, title);
    const list = document.createElement('ul');
    for (const mod of unresolved) {
      const item = document.createElement('li');
      const name = document.createElement('code');
      name.textContent = mod.technical_name;
      item.append(
        name,
        ` — ${dropped.has(mod.technical_name) ? 'removed: ' : ''}${this.#unresolvedReason(mod, odoo_ver)}`,
      );
      if (mod.url) {
        const link = document.createElement('a');
        link.href = mod.url;
        link.target = '_blank';
        link.rel = 'noopener';
        link.textContent = 'PR/MR';
        item.append(' (', link, ')');
      }
      list.append(item);
    }
    section.append(head, list);
    this.#el_unresolved.append(section);
  }

  #fillOdooVersionsSearchOptions() {
    this.#el_search_select_ver.replaceChildren();
    this.getFetchData('odoo_versions')
      .map(({value}) => new Option(value))
      .forEach(option => this.#el_search_select_ver.add(option));
  }

  #readFileAsText(file) {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        resolve(reader.result);
      };
      reader.onerror = err => {
        reject(err);
      };
      reader.readAsText(file);
    });
  }

  // Reads, parses and posts `file` in one guarded path so a malformed
  // addons.yaml or a request failure both surface the same way instead of
  // one of them silently doing nothing.
  async #processFile(file) {
    setDragPanelProcessing(this.#el_drag_panel, true);
    try {
      const file_text = await this.#readFileAsText(file);
      const yaml_data = yaml.load(file_text);
      const odoo_ver =
        this.#el_search_select_ver.value ||
        this.getFetchData('odoo_versions')[0].value;
      const formData = new FormData();
      formData.append('odoo_version', odoo_ver);
      // Module names only: not wildcards ("*", "web_*") nor ENV/ONLY maps.
      Object.values(yaml_data)
        .filter(Array.isArray)
        .flat()
        .filter(name => typeof name === 'string' && !/[*?[]/.test(name))
        .forEach(mod_name => formData.append('modules', mod_name));
      const data = await getService('requests').post(
        '/doodba/dependency-resolver/addons',
        {
          body: formData,
        },
      );
      if (!data.ok) {
        throw new Error(t('Server responded with {0}', data.status));
      }
      const json_data = await data.json();
      const dropped = new Map(
        json_data.unresolved
          .filter(mod => DROPPED.has(mod.status))
          .map(mod => [
            mod.technical_name,
            `${mod.technical_name}: ${this.#unresolvedReason(mod, odoo_ver)}`,
          ]),
      );
      const yaml_txt = this.#makeYaml(json_data.odoo, yaml_data, dropped);
      this.#showResults(
        yaml_txt,
        json_data.pip.join('\n'),
        json_data.bin.join('\n'),
      );
      renderLicenseReview(this.#el_license, json_data.license_review);
      this.#renderUnresolved(json_data.unresolved, dropped, odoo_ver);
      this.#el_search_select_ver.disabled = true;
      setDragPanelProcessing(this.#el_drag_panel, false);
    } catch (_err) {
      showDragPanelError(
        this.#el_drag_panel,
        'Could not process the file. Check its content and try again.',
      );
    }
  }

  async onDrop(ev) {
    ev.preventDefault();
    const file = ev.dataTransfer.items[0]?.getAsFile();
    if (isYamlFile(file)) {
      this.#processFile(file);
    } else {
      showDragPanelError(this.#el_drag_panel, 'Please drop a .yaml/.yml file.');
    }
  }

  async onClick(ev) {
    ev.preventDefault();
    const elem = window.document.createElement('input');
    elem.type = 'file';
    elem.hidden = true;
    elem.setAttribute('accept', '.yaml,.yml');
    elem.addEventListener('change', ev_chg => {
      const file = ev_chg.target.files[0];
      if (isYamlFile(file)) {
        this.#processFile(file);
      }
    });
    document.body?.appendChild(elem);
    elem.click();
    document.body?.removeChild(elem);
  }
}

registerComponent('doodba-dependency-resolver', DoodbaDependencyResolver);
