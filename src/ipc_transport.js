// Reject Tauri's ipc:// fetch at once so invoke() goes straight to its
// postMessage fallback. On a remote https page the fetch fails anyway, and
// Tauri falls back without this file. The guard only stops the failed fetch
// from reaching the page's CSP reporting. Commit 81ef66e added it for Google
// sign-in; docs/handoff/ipc-transport-and-spoof.md records why that is unproven.
(function() {
  if (window.__ytm_ipc_guard_applied) return;
  window.__ytm_ipc_guard_applied = true;

  var originalFetch = window.fetch;
  window.fetch = function(url) {
    if (typeof url === 'string' && url.startsWith('ipc://')) {
      return Promise.reject(new TypeError('IPC blocked on remote pages'));
    }
    return originalFetch.apply(this, arguments);
  };
})();
