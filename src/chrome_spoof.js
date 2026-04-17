// Spoof Chrome environment for YouTube Music and Google OAuth compatibility.
// Also forces postMessage IPC transport (ipc:// is blocked on external URLs).
(function() {
  if (window.__ytm_spoof_applied) return;
  window.__ytm_spoof_applied = true;

  var CHROME_VERSION = '125';
  var CHROME_FULL_VERSION = '125.0.6422.142';

  var originalFetch = window.fetch;
  window.fetch = function(url) {
    if (typeof url === 'string' && url.startsWith('ipc://')) {
      return Promise.reject(new TypeError('IPC blocked on remote pages'));
    }
    return originalFetch.apply(this, arguments);
  };

  if (!window.chrome) {
    window.chrome = {
      runtime: { connect: function() {}, sendMessage: function() {} },
      app: {
        isInstalled: false,
        InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' },
        RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' },
      },
      csi: function() { return {}; },
      loadTimes: function() {
        var now = Date.now() / 1000;
        return {
          commitLoadTime: now, connectionInfo: 'http/1.1',
          finishDocumentLoadTime: now, finishLoadTime: now,
          firstPaintAfterLoadTime: 0, firstPaintTime: now,
          navigationType: 'Other', npnNegotiatedProtocol: 'unknown',
          requestTime: now, startLoadTime: now,
          wasAlternateProtocolAvailable: false,
          wasFetchedViaSpdy: false, wasNpnNegotiated: false,
        };
      },
    };
  }

  if (!navigator.userAgentData) {
    var brands = [
      { brand: 'Chromium', version: CHROME_VERSION },
      { brand: 'Google Chrome', version: CHROME_VERSION },
      { brand: 'Not-A.Brand', version: '24' },
    ];
    Object.defineProperty(navigator, 'userAgentData', {
      value: {
        brands: brands,
        mobile: false,
        platform: 'macOS',
        getHighEntropyValues: function() {
          return Promise.resolve({
            architecture: 'arm', bitness: '64', brands: brands,
            fullVersionList: [
              { brand: 'Chromium', version: CHROME_FULL_VERSION },
              { brand: 'Google Chrome', version: CHROME_FULL_VERSION },
              { brand: 'Not-A.Brand', version: '24.0.0.0' },
            ],
            mobile: false, model: '', platform: 'macOS',
            platformVersion: '15.0.0', uaFullVersion: CHROME_FULL_VERSION,
          });
        },
        toJSON: function() {
          return { brands: brands, mobile: false, platform: 'macOS' };
        },
      },
      configurable: true, enumerable: true,
    });
  }

  Object.defineProperty(navigator, 'plugins', {
    get: function() {
      return [
        { name: 'PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
        { name: 'Chrome PDF Viewer', filename: 'internal-pdf-viewer', description: '' },
        { name: 'Chromium PDF Viewer', filename: 'internal-pdf-viewer', description: '' },
      ];
    },
  });

  Object.defineProperty(navigator, 'webdriver', {
    get: function() { return false; },
  });
})();
