// Copyright 2025 Alexandre D. Díaz
import {Component, registerComponent, getService, HTTP_METHOD} from 'mirlo';
import yaml from 'js-yaml';
import '@scss/components/doodba-converter.scss';

const MANIFEST_NAMES = ['__manifest__.py', '__openerp__.py'];

class DoodbaConverter extends Component {
  #el_search_select_ver = null;
  #el_drag_panel = null;
  #el_result = null;
  #el_save = null;

  onSetup() {
    Component.useEvents({
      doodba_converter_drag_panel: {
        mode: 'id',
        events: {
          dragenter: this.onDragEnter,
          dragover: this.onDragOver,
          dragleave: this.onDragLeave,
          drop: this.onDrop,
          click: this.onClick,
        },
      },
      doodba_converter_save: {
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
      'doodba_converter_search_select_ver',
    );
    this.#el_drag_panel = this.queryId('doodba_converter_drag_panel');
    this.#el_result = this.queryId('doodba_converter_result');
    this.#el_save = this.queryId('doodba_converter_save');
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

  onClickSave() {
    const blob = new Blob([this.#el_result.value], {type: 'text/yaml'});
    const elem = window.document.createElement('a');
    const objURL = window.URL.createObjectURL(blob);
    elem.href = objURL;
    elem.download = 'addons.yaml';
    document.body?.appendChild(elem);
    elem.click();
    document.body?.removeChild(elem);
    URL.revokeObjectURL(objURL);
  }

  #showYaml(yaml_data) {
    this.#el_result.value = yaml_data;
    this.#el_drag_panel.style.display = 'none';
    this.#el_result.style.display = '';
    this.#el_save.style.display = '';
  }

  #makeYaml(data, mods) {
    const groupedData = data.reduce((acc, item) => {
      const {technical_name, repository_name} = item;
      if (!acc[repository_name]) {
        acc[repository_name] = [];
      }
      acc[repository_name].push(technical_name);
      return acc;
    }, {});
    const data_mods = data.map(mod_info => mod_info.technical_name);
    const difference = mods.filter(x => !data_mods.includes(x));
    if (difference.length > 0) {
      groupedData['_UNKNOWN_'] = difference;
    }

    const sortedGroupedData = Object.keys(groupedData)
      .sort()
      .reduce((acc, key) => {
        acc[key] = groupedData[key];
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

  async #readAddonFolder(parent_name, entries, acc_res) {
    if (typeof acc_res === 'undefined') {
      acc_res = [];
    }
    if (this.#hasManifest(entries)) {
      acc_res.push(parent_name);
    } else {
      for (const entry of entries) {
        try {
          const n_entries = await this.#readDirectoryEntries(entry);
          await this.#readAddonFolder(entry.name, n_entries, acc_res);
        } catch (_err) {
          // do nothing
        }
      }
    }
    return acc_res;
  }

  async #processAddons(module_names) {
    const mods = module_names.filter(
      (value, index, array) => array.indexOf(value) === index,
    );
    if (mods.length > 0) {
      const odoo_ver =
        this.#el_search_select_ver.value ||
        this.getFetchData('odoo_versions')[0].value;

      const formData = new FormData();
      formData.append('odoo_version', odoo_ver);
      mods.forEach(mod_name => formData.append('modules', mod_name));
      const data = await getService('requests').post(
        '/doodba/converter/addons',
        {
          body: formData,
        },
      );
      const json_data = await data.json();
      const yaml_txt = this.#makeYaml(json_data, mods);
      this.#showYaml(yaml_txt);
      this.#el_search_select_ver.disabled = true;
    }
  }

  async onDrop(ev) {
    ev.preventDefault();
    const mods = [];
    for (const raw_item of ev.dataTransfer.items) {
      const entry = raw_item.webkitGetAsEntry();
      try {
        const entries = await this.#readDirectoryEntries(entry);
        mods.push(...(await this.#readAddonFolder(entry.name, entries)));
      } catch (_err) {
        // do nothing
      }
    }
    this.#processAddons(mods);
  }

  async onClick(ev) {
    ev.preventDefault();
    const elem = window.document.createElement('input');
    elem.type = 'file';
    elem.hidden = true;
    elem.setAttribute('accept', '.py');
    elem.setAttribute('directory', true);
    elem.setAttribute('webkitdirectory', true);
    elem.addEventListener('change', ev_chg => {
      const files = Array.from(ev_chg.target.files);
      const subfolders = [];
      files.forEach(file => {
        const relativePath = file.webkitRelativePath || '';
        const parts = relativePath.split('/').filter(part => part !== '');
        if (parts.length > 1 && MANIFEST_NAMES.includes(parts.at(-1))) {
          const folderName = parts.at(-2);
          if (!subfolders.includes(folderName)) {
            subfolders.push(folderName);
          }
        }
      });
      this.#processAddons(subfolders);
    });
    document.body?.appendChild(elem);
    elem.click();
    document.body?.removeChild(elem);

    // const handle = await showDirectoryPicker({
    //   mode: "read",
    // });
    // const mods = [];
    // const items = handle.values();
    // for await (const entry of items) {
    //   try {
    //     const entries = await this.#readDirectoryEntries(entry);
    //     mods.push(...(await this.#readAddonFolder(entry.name, entries)));
    //   } catch (_err) {
    //     // do nothing
    //   }
    // }
    // this.#processAddons(mods);
  }
}

registerComponent('doodba-converter', DoodbaConverter);
