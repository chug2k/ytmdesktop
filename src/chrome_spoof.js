// Spoof Chrome-specific APIs so Google's OAuth and YouTube Music
// accept this WKWebView as a real Chrome browser.
(function() {
  // Chrome object — Google checks for window.chrome
  if (!window.chrome) {
    window.chrome = {
      runtime: {
        connect: function() {},
        sendMessage: function() {},
      },
      app: {
        isInstalled: false,
        InstallState: { DISABLED: 'disabled', INSTALLED: 'installed', NOT_INSTALLED: 'not_installed' },
        RunningState: { CANNOT_RUN: 'cannot_run', READY_TO_RUN: 'ready_to_run', RUNNING: 'running' },
      },
      csi: function() { return {}; },
      loadTimes: function() {
        return {
          commitLoadTime: Date.now() / 1000,
          connectionInfo: 'http/1.1',
          finishDocumentLoadTime: Date.now() / 1000,
          finishLoadTime: Date.now() / 1000,
          firstPaintAfterLoadTime: 0,
          firstPaintTime: Date.now() / 1000,
          navigationType: 'Other',
          npnNegotiatedProtocol: 'unknown',
          requestTime: Date.now() / 1000,
          startLoadTime: Date.now() / 1000,
          wasAlternateProtocolAvailable: false,
          wasFetchedViaSpdy: false,
          wasNpnNegotiated: false,
        };
      },
    };
  }

  // Navigator Client Hints API — Chrome sends Sec-CH-UA headers and
  // exposes navigator.userAgentData. Google's auth checks this.
  if (!navigator.userAgentData) {
    Object.defineProperty(navigator, 'userAgentData', {
      value: {
        brands: [
          { brand: 'Chromium', version: '125' },
          { brand: 'Google Chrome', version: '125' },
          { brand: 'Not-A.Brand', version: '24' },
        ],
        mobile: false,
        platform: 'macOS',
        getHighEntropyValues: function(hints) {
          return Promise.resolve({
            architecture: 'arm',
            bitness: '64',
            brands: this.brands,
            fullVersionList: [
              { brand: 'Chromium', version: '125.0.6422.142' },
              { brand: 'Google Chrome', version: '125.0.6422.142' },
              { brand: 'Not-A.Brand', version: '24.0.0.0' },
            ],
            mobile: false,
            model: '',
            platform: 'macOS',
            platformVersion: '15.0.0',
            uaFullVersion: '125.0.6422.142',
          });
        },
        toJSON: function() {
          return { brands: this.brands, mobile: this.mobile, platform: this.platform };
        },
      },
      configurable: true,
      enumerable: true,
    });
  }

  // Plugin/MimeType arrays — Chrome has these populated
  Object.defineProperty(navigator, 'plugins', {
    get: function() {
      return [
        { name: 'PDF Viewer', filename: 'internal-pdf-viewer', description: 'Portable Document Format' },
        { name: 'Chrome PDF Viewer', filename: 'internal-pdf-viewer', description: '' },
        { name: 'Chromium PDF Viewer', filename: 'internal-pdf-viewer', description: '' },
      ];
    },
  });

  // Ensure webdriver is false (Google checks this to detect automation)
  Object.defineProperty(navigator, 'webdriver', {
    get: function() { return false; },
  });
})();
