// Copyright (C) 2024 Alexandre Díaz
import {Component, registerComponent, getService, HTTP_METHOD} from 'mirlo';
import yaml from 'js-yaml';
import '@scss/components/doodba-converter.scss';


const MANIFEST_NAMES = ["__manifest__.py"];

class DoodbaConverter extends Component {
  #drag_panel = null;
  #result = null;

  onSetup() {
    Component.useStyles('/static/auto/web/scss/components/doodba-converter.css');
    Component.useEvents({
      drag_panel: {
        mode: 'id',
        events: {
          dragenter: this.onDragEnter,
          dragover: this.onDragOver,
          dragleave: this.onDragLeave,
          drop: this.onDrop,
        },
      },
    });
  }

  onStart() {
    super.onStart();
    this.#drag_panel = this.queryId('drag_panel');
    this.#result = this.queryId('result');
  }

  showYaml(yaml_data) {
    this.#result.value = yaml_data;
    this.#drag_panel.style.display = 'none';
    this.#result.style.display = '';
  }

  onDragEnter(ev) {
    ev.preventDefault();
    this.#drag_panel.style.backgroundColor = '#54555b';
  }

  onDragOver(ev) {
    ev.preventDefault();
  }

  onDragLeave(ev) {
    ev.preventDefault();
    this.#drag_panel.style.backgroundColor = '';
  }

  makeYaml(data, mods) {
    const groupedData = data.reduce((acc, item) => {
      const { technical_name, repository_name } = item;
      if (!acc[repository_name]) {
          acc[repository_name] = [];
      }
      acc[repository_name].push(technical_name);
      return acc;
    }, {});

    const sortedGroupedData = Object.keys(groupedData)
        .sort()
        .reduce((acc, key) => {
            acc[key] = groupedData[key];
            return acc;
        }, {});

    const data_mods = data.map((mod_info) => mod_info.technical_name);
    const difference = mods.filter(x => !data_mods.includes(x));
    sortedGroupedData['unknown'] = difference;

    return yaml.dump(sortedGroupedData, { indent: 2 });
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
        dir_reader.readEntries(resolve);
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

  async onDrop(ev) {
    ev.preventDefault();
    let mods = [];
    for (const raw_item of ev.dataTransfer.items) {
      const entry = raw_item.webkitGetAsEntry();
      try {
        const entries = await this.#readDirectoryEntries(entry);
        mods.push(...(await this.readAddonFolder(entry.name, entries)));
      } catch (_err) {
        // do nothing
      }
    }
    mods = mods.filter((value, index, array) => array.indexOf(value) === index)
    if (mods.length > 0) {
      const data = await getService('requests').postJSON(
        `/common/doodba/addons`,
        {
          modules: mods,
        }
      );
      const yaml = this.makeYaml(data, mods);
      this.showYaml(yaml);
    }
  }
}

registerComponent('doodba-converter', DoodbaConverter);
