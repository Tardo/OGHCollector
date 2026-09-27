// Copyright Alexandre D. Díaz
import '@app/components/committer-search';
import '@scss/pages/committer.scss';
import {bindSearchModal} from '@app/utils/search-modal';

bindSearchModal('committer_search', 'mirlo-committer-search');

const canvas = document.querySelector('#committer-landscape');
if (canvas) {
  const versions = [...document.querySelectorAll('[data-odoo-version]')].map(
    pane => ({
      label: pane.dataset.odooVersion,
      commits: [...pane.querySelectorAll('tr[data-repo]')].map(row => ({
        repo: row.dataset.repo,
        count: Number(row.dataset.commits),
      })),
    }),
  );
  const totals = new Map();
  for (const version of versions) {
    for (const {repo, count} of version.commits) {
      totals.set(repo, (totals.get(repo) || 0) + count);
    }
  }
  const repos = [...totals]
    .sort((a, b) => b[1] - a[1])
    .slice(0, 5)
    .map(([repo]) => repo);
  const legend = document.querySelector('#landscape-repos');
  repos.forEach((repo, index) => {
    const item = document.createElement('li');
    const swatch = document.createElement('span');
    swatch.style.backgroundColor = `hsl(${(index * 59 + 165) % 360} 78% 60%)`;
    swatch.setAttribute('aria-hidden', 'true');
    item.append(swatch, `${repo} · ${totals.get(repo)} commits`);
    legend.append(item);
  });
  const bars = versions
    .flatMap((version, x) =>
      repos.map((repo, y) => ({
        x: x - (versions.length - 1) / 2,
        y: y - (repos.length - 1) / 2,
        version: version.label,
        repo,
        count: version.commits
          .filter(item => item.repo === repo)
          .reduce((sum, item) => sum + item.count, 0),
        color: (y * 59 + 165) % 360,
      })),
    )
    .filter(bar => bar.count > 0);
  const ctx = canvas.getContext('2d');
  const caption = document.querySelector('#landscape-caption');
  const max = Math.max(...bars.map(bar => bar.count), 1);
  let width = 0;
  let height = 0;
  let angle = -0.7;
  let dragging = null;
  let hovered = null;
  let targets = [];

  function project(x, y, z) {
    const depth = x * Math.sin(angle) + y * Math.cos(angle);
    const side = x * Math.cos(angle) - y * Math.sin(angle);
    const span = Math.hypot(versions.length, repos.length) / 2;
    const scale = Math.min(
      (width - 140) / (span * 2 + 1),
      (height * 0.6) / (span * 0.55 + 3),
    );
    return [
      width / 2 + side * scale,
      height * 0.66 + (depth * 0.55 - z * 0.84) * scale,
    ];
  }

  function face(points, fill) {
    const path = new Path2D();
    points.forEach(([x, y], index) =>
      index ? path.lineTo(x, y) : path.moveTo(x, y),
    );
    path.closePath();
    ctx.fillStyle = fill;
    ctx.fill(path);
    ctx.strokeStyle = 'rgba(255,255,255,.35)';
    ctx.lineWidth = 0.7;
    ctx.stroke(path);
    return path;
  }

  function draw() {
    ctx.clearRect(0, 0, width, height);
    if (!bars.length) return;
    ctx.fillStyle = '#102635';
    ctx.fillRect(0, 0, width, height);
    targets = [];
    const ordered = [...bars].sort(
      (a, b) =>
        a.x * Math.sin(angle) +
        a.y * Math.cos(angle) -
        (b.x * Math.sin(angle) + b.y * Math.cos(angle)),
    );
    for (const bar of ordered) {
      const z = 0.2 + 3 * Math.sqrt(bar.count / max);
      const corners = [
        [-0.36, -0.36],
        [0.36, -0.36],
        [0.36, 0.36],
        [-0.36, 0.36],
      ];
      const bottom = corners.map(([dx, dy]) =>
        project(bar.x + dx, bar.y + dy, 0),
      );
      const top = corners.map(([dx, dy]) => project(bar.x + dx, bar.y + dy, z));
      const highlight = bar === hovered ? 18 : 0;
      const faces = [];
      faces.push(
        face(
          [bottom[0], bottom[1], top[1], top[0]],
          `hsl(${bar.color} 62% ${32 + highlight}%)`,
        ),
      );
      faces.push(
        face(
          [bottom[1], bottom[2], top[2], top[1]],
          `hsl(${bar.color} 65% ${39 + highlight}%)`,
        ),
      );
      faces.push(
        face(
          [bottom[2], bottom[3], top[3], top[2]],
          `hsl(${bar.color} 62% ${33 + highlight}%)`,
        ),
      );
      faces.push(
        face(
          [bottom[3], bottom[0], top[0], top[3]],
          `hsl(${bar.color} 65% ${41 + highlight}%)`,
        ),
      );
      faces.push(face(top, `hsl(${bar.color} 78% ${60 + highlight}%)`));
      targets.push({bar, faces});
    }
    ctx.font = '12px system-ui, sans-serif';
    ctx.fillStyle = '#d4e7e9';
    ctx.textAlign = 'center';
    versions.forEach((version, index) => {
      const [x, y] = project(
        index - (versions.length - 1) / 2,
        (repos.length - 1) / 2 + 0.8,
        0,
      );
      ctx.fillText(version.label, x, y);
    });
    ctx.textAlign = 'right';
    repos.forEach((repo, index) => {
      const [x, y] = project(
        -(versions.length - 1) / 2 - 0.8,
        index - (repos.length - 1) / 2,
        0,
      );
      ctx.fillText(repo.length > 22 ? `…${repo.slice(-21)}` : repo, x, y);
    });
  }

  new ResizeObserver(() => {
    const dpr = Math.min(devicePixelRatio || 1, 2);
    width = canvas.clientWidth;
    height = canvas.clientHeight;
    canvas.width = Math.round(width * dpr);
    canvas.height = Math.round(height * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    draw();
  }).observe(canvas);
  canvas.addEventListener('pointerdown', event => {
    dragging = event.clientX;
    canvas.setPointerCapture(event.pointerId);
  });
  canvas.addEventListener('pointerup', () => {
    dragging = null;
  });
  canvas.addEventListener('pointercancel', () => {
    dragging = null;
  });
  canvas.addEventListener('pointerleave', () => {
    hovered = null;
    caption.textContent =
      'Drag to rotate the landscape; hover over a column for details.';
    draw();
  });
  canvas.addEventListener('pointermove', event => {
    if (dragging !== null) {
      angle += (event.clientX - dragging) * 0.008;
      dragging = event.clientX;
      draw();
      return;
    }
    const rect = canvas.getBoundingClientRect();
    const x = event.clientX - rect.left;
    const y = event.clientY - rect.top;
    hovered =
      [...targets]
        .reverse()
        .find(({faces}) => faces.some(shape => ctx.isPointInPath(shape, x, y)))
        ?.bar || null;
    caption.textContent = hovered
      ? `${hovered.repo} · Odoo ${hovered.version} · ${hovered.count} tracked commits`
      : 'Drag to rotate the landscape; hover over a column for details.';
    draw();
  });
}
