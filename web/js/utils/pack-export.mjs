// Copyright 2026 Alexandre D. Díaz
import * as yaml from 'js-yaml';
import JSZip from 'jszip';

// Groups the resolved modules by repo (adding a `_MISSING_` bucket for any
// pack module the chosen Odoo version doesn't have), then builds addons.yaml
// + repos.yaml (git remotes, skipped if no repo/org could be resolved) +
// pip.txt + apt.txt and triggers the .zip download.
export async function exportPackZip(pack, odoo_version) {
  const formData = new FormData();
  formData.append('odoo_version', odoo_version);
  for (const mod of pack.modules) {
    formData.append('modules', mod.technical_name);
  }
  const res = await fetch('/doodba/dependency-resolver/addons', {
    method: 'POST',
    body: formData,
  });
  if (!res.ok) {
    window.alert('Failed to resolve dependencies for this pack.');
    return;
  }
  const data = await res.json();

  const grouped = {...data.odoo};
  const resolved = new Set(Object.values(grouped).flat());
  const missing = pack.modules
    .map(m => m.technical_name)
    .filter(name => !resolved.has(name));
  if (missing.length > 0) {
    grouped._MISSING_ = missing;
  }
  if (Object.keys(grouped).length === 0) {
    window.alert(`No modules found for Odoo ${odoo_version} in this pack.`);
    return;
  }
  const sorted_odoo = Object.keys(grouped)
    .sort()
    .reduce((acc, key) => {
      acc[key] = [...grouped[key]].sort();
      return acc;
    }, {});

  const zip = new JSZip();
  zip.file('addons.yaml', yaml.dump(sorted_odoo, {indent: 2}));
  const repo_names = Object.keys(sorted_odoo).filter(name => data.repos[name]);
  if (repo_names.length > 0) {
    const repos = {};
    for (const repo_name of repo_names) {
      const org = data.repos[repo_name];
      repos[repo_name] = {
        remotes: {[org]: `https://github.com/${org}/${repo_name}.git`},
        target: `${org} ${odoo_version}`,
        merges: [`${org} ${odoo_version}`],
      };
    }
    zip.file('repos.yaml', yaml.dump(repos, {indent: 2}));
  }
  zip.file('pip.txt', data.pip.join('\n'));
  zip.file('apt.txt', data.bin.join('\n'));

  const zip_bin = await zip.generateAsync({type: 'blob'});
  const elem = document.createElement('a');
  const objURL = window.URL.createObjectURL(zip_bin);
  elem.href = objURL;
  elem.download = `${pack.name.replace(/[^a-z0-9_-]+/gi, '_') || 'pack'}.zip`;
  document.body.appendChild(elem);
  elem.click();
  document.body.removeChild(elem);
  URL.revokeObjectURL(objURL);
}
