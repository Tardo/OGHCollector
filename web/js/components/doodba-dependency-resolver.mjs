// Copyright 2025 Alexandre D. Díaz
import {Component, registerComponent, getService, HTTP_METHOD} from 'mirlo';
import yaml from 'js-yaml';
import JSZip from 'jszip';
import '@scss/components/doodba-dependency-resolver.scss';

const MANIFEST_NAMES = ['__manifest__.py'];

class DoodbaDependencyResolver extends Component {
  #el_search_select_ver = null;
  #el_drag_panel = null;
  #el_result_odoo = null;
  #el_result_pip = null;
  #el_result_bin = null;
  #el_save = null;

  onSetup() {
    Component.useStyles(
      '/static/auto/web/scss/components/doodba-dependency-resolver.css',
    );
    Component.useEvents({
      doodba_dep_resolver_drag_panel: {
        mode: 'id',
        events: {
          dragenter: this.onDragEnter,
          dragover: this.onDragOver,
          dragleave: this.onDragLeave,
          drop: this.onDrop,
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
    this.#el_result_odoo = this.queryId('doodba_dep_resolver_result_odoo');
    this.#el_result_pip = this.queryId('doodba_dep_resolver_result_pip');
    this.#el_result_bin = this.queryId('doodba_dep_resolver_result_bin');
    this.#el_save = this.queryId('doodba_dep_resolver_save');
  }

  onStart() {
    super.onStart();
    this.#fillOdooVersionsSearchOptions();
  }

  showResults(yaml_data, pip_data, bin_data) {
    this.#el_result_odoo.value = yaml_data;
    this.#el_result_pip.value = pip_data;
    this.#el_result_bin.value = bin_data;
    this.#el_drag_panel.style.display = 'none';
    this.#el_result_odoo.style.display = '';
    this.#el_result_pip.style.display = '';
    this.#el_result_bin.style.display = '';
    this.#el_save.style.display = '';
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

  makeYaml(data, mods) {
    const sortedGroupedData = Object.keys(data)
      .sort()
      .reduce((acc, key) => {
        acc[key] = data[key];
        return acc;
      }, {});

    const data_mods = Object.values(data);
    const difference = mods.filter(x => !data_mods.includes(x));
    if (difference.length > 0) {
      sortedGroupedData['unknown'] = difference;
    }

    return yaml.dump(sortedGroupedData, {indent: 2});
  }

  #fillOdooVersionsSearchOptions() {
    this.#el_search_select_ver.replaceChildren();
    this.getFetchData('odoo_versions')
      .map(({value}) => new Option(value))
      .forEach(option => this.#el_search_select_ver.add(option));
  }

  #hasManifest(entries) {
    for (const entry of entries) {
      if (entry.isFile && MANIFEST_NAMES.includes(entry.name)) {
        return true;
      }
    }
    return false;
  }

  #readDirectoryEntries(entry) {
    return new Promise((resolve, reject) => {
      if (entry && entry.isDirectory) {
        const dir_reader = entry.createReader();
        const allEntries = [];

        function readEntries() {
          dir_reader.readEntries(entries => {
            if (entries.length === 0) {
              return resolve(allEntries);
            }
            allEntries.push(...entries);
            readEntries();
          }, reject);
        }

        readEntries();
      } else {
        reject();
      }
    });
  }

  async readAddonFolder(parent_name, entries, acc_res) {
    if (typeof acc_res === 'undefined') {
      acc_res = [];
    }
    if (this.#hasManifest(entries)) {
      acc_res.push(parent_name);
    } else {
      for (const entry of entries) {
        try {
          const n_entries = await this.#readDirectoryEntries(entry);
          await this.readAddonFolder(entry.name, n_entries, acc_res);
        } catch (_err) {
          // do nothing
        }
      }
    }
    return acc_res;
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

  async onDrop(ev) {
    ev.preventDefault();
    const raw_item = ev.dataTransfer.items[0];
    const file = raw_item.getAsFile();
    if (file.type.endsWith('/yaml')) {
      const file_text = await this.#readFileAsText(file);
      const mods = Object.values(yaml.load(file_text)).flat();
      const odoo_ver =
        this.#el_search_select_ver.value ||
        this.getFetchData('odoo_versions')[0].value;
      const formData = new FormData();
      formData.append('odoo_version', odoo_ver);
      mods.forEach(mod_name => formData.append('modules', mod_name));
      const data = await getService('requests').post(
        '/doodba/dependency-resolver/addons',
        {
          headers: undefined,
          body: formData,
        },
      );
      const json_data = await data.json();
      const yaml_txt = this.makeYaml(json_data.odoo, mods);
      this.showResults(
        yaml_txt,
        json_data.bin.join('\n'),
        json_data.pip.join('\n'),
      );
      this.#el_search_select_ver.disabled = true;
    }
  }
}

registerComponent('doodba-dependency-resolver', DoodbaDependencyResolver);
