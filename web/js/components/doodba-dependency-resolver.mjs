// Copyright 2025 Alexandre D. Díaz
import {Component, registerComponent, getService, HTTP_METHOD} from 'mirlo';
import * as yaml from 'js-yaml';
import JSZip from 'jszip';
import {
  setDragPanelProcessing,
  showDragPanelError,
  isYamlFile,
} from '@app/utils/drag-panel-processing';
import '@scss/components/doodba-dependency-resolver.scss';

class DoodbaDependencyResolver extends Component {
  #el_search_select_ver = null;
  #el_drag_panel = null;
  #el_result = null;
  #el_result_odoo = null;
  #el_result_pip = null;
  #el_result_bin = null;
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
    this.#el_save = this.queryId('doodba_dep_resolver_save');
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

  #makeYaml(data, yaml_data) {
    const data_mods = Object.values(data).flat();
    const yaml_mods = Object.values(yaml_data).flat();
    const diff_mods = yaml_mods.filter(x => !data_mods.includes(x));
    const san_diff_mods = [];
    const mods_entries = Object.entries(yaml_data);
    for (const mod_name of diff_mods) {
      let found = false;
      for (const [repo_name, mod_names] of mods_entries) {
        if (mod_names.includes(mod_name)) {
          found = true;
          if (!Object.hasOwn(data, repo_name)) {
            data[repo_name] = [mod_name];
          } else {
            data[repo_name].push(mod_name);
          }
          break;
        }
      }
      if (!found) {
        san_diff_mods.push(mod_name);
      }
    }
    if (san_diff_mods.length > 0) {
      data['_UNKNOWN_'] = diff_mods;
    }

    const sortedGroupedData = Object.keys(data)
      .sort()
      .reduce((acc, key) => {
        acc[key] = data[key];
        return acc;
      }, {});
    return yaml.dump(sortedGroupedData, {indent: 2});
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
      Object.values(yaml_data)
        .flat()
        .forEach(mod_name => formData.append('modules', mod_name));
      const data = await getService('requests').post(
        '/doodba/dependency-resolver/addons',
        {
          body: formData,
        },
      );
      if (!data.ok) {
        throw new Error(`Server responded with ${data.status}`);
      }
      const json_data = await data.json();
      const yaml_txt = this.#makeYaml(json_data.odoo, yaml_data);
      this.#showResults(
        yaml_txt,
        json_data.bin.join('\n'),
        json_data.pip.join('\n'),
      );
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
