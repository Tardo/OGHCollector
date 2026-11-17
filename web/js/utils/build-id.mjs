// Copyright 2026 Alexandre D. Díaz
// Pages are cached by the browser for `cache_ttl`, but the scripts revalidate:
// after a deploy a cached page can meet a newer bundle that expects other
// markup. Every page names its build in <meta name="build-id"> (the server
// reads static/auto/build-id.txt); when it differs from this bundle's, reload
// once, which makes the browser revalidate the page itself.
import BUILD_ID from 'virtual:build-id';

const meta = document.querySelector('meta[name="build-id"]')?.content;
if (meta && meta !== BUILD_ID) {
  try {
    if (sessionStorage.getItem('ommd_build_reload') !== BUILD_ID) {
      sessionStorage.setItem('ommd_build_reload', BUILD_ID);
      location.reload();
    }
  } catch {
    // No sessionStorage: skip rather than risk a reload loop.
  }
}
