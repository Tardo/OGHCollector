// Copyright 2026 Alexandre D. Díaz
import {Component, registerComponent, getService, HTTP_METHOD} from 'mirlo';
import * as yaml from 'js-yaml';
import JSZip from 'jszip';
import '@scss/components/doodba-migration-plan.scss';

class DoodbaMigrationPlan extends Component {
  #el_search_select_from = null;
  #el_search_select_to = null;
  #el_drag_panel = null;
  #el_result = null;
  #el_save = null;
  #steps = [];

  onSetup() {
    Component.useEvents({
      doodba_migration_plan_drag_panel: {
        mode: 'id',
        events: {
          dragenter: this.onDragEnter,
          dragover: this.onDragOver,
          dragleave: this.onDragLeave,
          drop: this.onDrop,
          click: this.onClick,
        },
      },
      doodba_migration_plan_save: {
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
    this.#el_search_select_from = this.queryId(
      'doodba_migration_plan_search_select_from',
    );
    this.#el_search_select_to = this.queryId(
      'doodba_migration_plan_search_select_to',
    );
    this.#el_drag_panel = this.queryId('doodba_migration_plan_drag_panel');
    this.#el_result = this.queryId('doodba_migration_plan_result');
    this.#el_save = this.queryId('doodba_migration_plan_save');
  }

  onStart() {
    super.onStart();
    this.#fillOdooVersionsSearchOptions();
  }

  onDragEnter(ev) {
    ev.preventDefault();
    this.#el_drag_panel.style.backgroundColor = '#54555b';
  }

  onDragOver(ev) {
    ev.preventDefault();
  }

  onDragLeave(ev) {
    ev.preventDefault();
    this.#el_drag_panel.style.backgroundColor = '';
  }

  async onClickSave() {
    const zip = new JSZip();
    for (const step of this.#steps) {
      const folder = zip.folder(step.version);
      folder.file('addons.yaml', this.#makeAddonsYaml(step));
      if (step.pending.length > 0) {
        folder.file('repos.yaml', this.#makeReposYaml(step));
      }
    }
    const zip_bin = await zip.generateAsync({type: 'blob'});
    const elem = window.document.createElement('a');
    const objURL = window.URL.createObjectURL(zip_bin);
    elem.href = objURL;
    elem.download = 'doodba_migration_plan.zip';
    document.body?.appendChild(elem);
    elem.click();
    document.body?.removeChild(elem);
    URL.revokeObjectURL(objURL);
  }

  #makeAddonsYaml(step) {
    const grouped = {};
    for (const {technical_name, repository_name} of step.merged) {
      if (!grouped[repository_name]) {
        grouped[repository_name] = [];
      }
      grouped[repository_name].push(technical_name);
    }
    for (const {technical_name, repository_name} of step.pending) {
      if (!grouped[repository_name]) {
        grouped[repository_name] = [];
      }
      grouped[repository_name].push(technical_name);
    }
    if (step.missing.length > 0) {
      grouped._MISSING_ = [...step.missing];
    }
    const sorted = Object.keys(grouped)
      .sort()
      .reduce((acc, key) => {
        acc[key] = grouped[key].sort();
        return acc;
      }, {});
    return yaml.dump(sorted, {indent: 2});
  }

  #makeReposYaml(step) {
    const repos = {};
    for (const pr of step.pending) {
      if (!repos[pr.repository_name]) {
        repos[pr.repository_name] = {
          organization: pr.organization,
          prids: new Set(),
        };
      }
      repos[pr.repository_name].prids.add(pr.prid);
    }
    const result = {};
    for (const repo_name of Object.keys(repos).sort()) {
      const {organization, prids} = repos[repo_name];
      result[repo_name] = {
        remotes: {
          [organization]: `https://github.com/${organization}/${repo_name}.git`,
        },
        target: `${organization} ${step.version}`,
        merges: [
          `${organization} ${step.version}`,
          ...Array.from(prids)
            .sort((a, b) => a - b)
            .map(prid => `${organization} refs/pull/${prid}/head`),
        ],
      };
    }
    return yaml.dump(result, {indent: 2});
  }

  #ciStatusMarker(ci_status) {
    if (ci_status === 'success') return '✅';
    if (ci_status === 'pending') return '⏳';
    if (ci_status === 'failure') return '❌';
    return '';
  }

  #buildStepCard(step) {
    const card = document.createElement('div');
    card.classList.add('migration-step');

    const title = document.createElement('h4');
    title.textContent = `Odoo ${step.version}`;
    card.appendChild(title);

    const columns = document.createElement('div');
    columns.classList.add('migration-step-columns');

    const addons_col = document.createElement('div');
    addons_col.classList.add('migration-step-col');
    const addons_label = document.createElement('label');
    addons_label.textContent = 'addons.yaml';
    const addons_textarea = document.createElement('textarea');
    addons_textarea.readOnly = true;
    addons_textarea.value = this.#makeAddonsYaml(step);
    addons_col.append(addons_label, addons_textarea);

    const repos_col = document.createElement('div');
    repos_col.classList.add('migration-step-col');
    const repos_label = document.createElement('label');
    repos_label.textContent = 'repos.yaml';
    const repos_textarea = document.createElement('textarea');
    repos_textarea.readOnly = true;
    repos_textarea.value =
      step.pending.length > 0
        ? this.#makeReposYaml(step)
        : '# No pending migration PRs/MRs for this version.\n';
    repos_col.append(repos_label, repos_textarea);

    columns.append(addons_col, repos_col);
    card.appendChild(columns);

    if (step.pending.length > 0) {
      const pending_info = document.createElement('div');
      pending_info.classList.add('migration-step-pending');
      pending_info.textContent = `⏳ Pending PR/MR: ${step.pending
        .map(
          p =>
            `${p.technical_name} (#${p.prid} ${this.#ciStatusMarker(p.ci_status)})`,
        )
        .join(', ')}`;
      card.appendChild(pending_info);
    }

    if (step.missing.length > 0) {
      const missing_info = document.createElement('div');
      missing_info.classList.add('migration-step-missing');
      missing_info.textContent = `❌ Missing (no merge, no open PR): ${step.missing.join(', ')}`;
      card.appendChild(missing_info);
    }

    return card;
  }

  #showResults(steps) {
    this.#steps = steps;
    this.#el_result.replaceChildren();
    if (steps.length === 0) {
      const empty = document.createElement('div');
      empty.textContent =
        'No Odoo versions found in this range. Check the selected "From"/"To" versions.';
      this.#el_result.appendChild(empty);
    } else {
      steps.forEach(step =>
        this.#el_result.appendChild(this.#buildStepCard(step)),
      );
    }
    this.#el_drag_panel.style.display = 'none';
    this.#el_result.style.display = '';
    this.#el_save.style.display = '';
  }

  #fillOdooVersionsSearchOptions() {
    this.#el_search_select_from.replaceChildren();
    while (this.#el_search_select_to.options.length > 1) {
      this.#el_search_select_to.remove(1);
    }
    this.getFetchData('odoo_versions').forEach(({value}) => {
      this.#el_search_select_from.add(new Option(value));
      this.#el_search_select_to.add(new Option(value));
    });
    // odoo_versions is newest-first; default "From" to the oldest version,
    // since planning a jump from the newest version to itself is a no-op.
    this.#el_search_select_from.selectedIndex =
      this.#el_search_select_from.options.length - 1;
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

  async #processYAML(yaml_data) {
    const modules = Object.values(yaml_data).flat();
    if (modules.length === 0) {
      return;
    }
    const from_ver =
      this.#el_search_select_from.value ||
      this.getFetchData('odoo_versions')[0].value;
    const to_ver = this.#el_search_select_to.value;
    const formData = new FormData();
    formData.append('from_version', from_ver);
    if (to_ver) {
      formData.append('to_version', to_ver);
    }
    modules.forEach(mod_name => formData.append('modules', mod_name));
    const data = await getService('requests').post('/doodba/migration/plan', {
      body: formData,
    });
    const steps = await data.json();
    this.#showResults(steps);
    this.#el_search_select_from.disabled = true;
    this.#el_search_select_to.disabled = true;
  }

  async onDrop(ev) {
    ev.preventDefault();
    const raw_item = ev.dataTransfer.items[0];
    const file = raw_item.getAsFile();
    if (file.type.endsWith('/yaml')) {
      const file_text = await this.#readFileAsText(file);
      this.#processYAML(yaml.load(file_text));
    }
  }

  async onClick(ev) {
    ev.preventDefault();
    const elem = window.document.createElement('input');
    elem.type = 'file';
    elem.hidden = true;
    elem.setAttribute('accept', '.yaml,.yml');
    elem.addEventListener('change', async ev_chg => {
      const file = ev_chg.target.files[0];
      if (file.type.endsWith('/yaml')) {
        const file_text = await this.#readFileAsText(file);
        this.#processYAML(yaml.load(file_text));
      }
    });
    document.body?.appendChild(elem);
    elem.click();
    document.body?.removeChild(elem);
  }
}

registerComponent('doodba-migration-plan', DoodbaMigrationPlan);
