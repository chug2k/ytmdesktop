// Passkey sign-in with a phone, for Google's sign-in page only.
//
// WKWebView rejects navigator.credentials.get for google.com at once (Apple
// gives that entitlement only to browsers), and Google then says "make sure
// Bluetooth is on". This script sends the request to the app instead, which
// talks to the phone the way Chrome does: QR code, Bluetooth, tunnel server.
// See src-tauri/src/passkey/mod.rs.
(function() {
  if (window.__ytm_passkey_applied) return;
  window.__ytm_passkey_applied = true;
  if (location.hostname !== 'accounts.google.com' || window.top !== window) return;
  if (!window.CredentialsContainer || !window.PublicKeyCredential) return;

  var nativeGet = CredentialsContainer.prototype.get;

  function invoke(cmd, args) {
    return window.__TAURI_INTERNALS__.invoke(cmd, args);
  }

  function toBytes(source) {
    if (source instanceof ArrayBuffer) return new Uint8Array(source);
    return new Uint8Array(source.buffer, source.byteOffset, source.byteLength);
  }

  function b64(source) {
    var bytes = toBytes(source), s = '';
    for (var i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
    return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
  }

  function unb64(value) {
    var s = atob(value.replace(/-/g, '+').replace(/_/g, '/'));
    var bytes = new Uint8Array(s.length);
    for (var i = 0; i < s.length; i++) bytes[i] = s.charCodeAt(i);
    return bytes.buffer;
  }

  function define(target, props) {
    Object.keys(props).forEach(function(key) {
      Object.defineProperty(target, key, { value: props[key], enumerable: true });
    });
    return target;
  }

  // Own properties shadow the native getters, which would throw on an
  // object the engine did not create.
  function toCredential(c) {
    var response = define(Object.create(AuthenticatorAssertionResponse.prototype), {
      clientDataJSON: unb64(c.clientDataJson),
      authenticatorData: unb64(c.authenticatorData),
      signature: unb64(c.signature),
      userHandle: c.userHandle ? unb64(c.userHandle) : null
    });
    return define(Object.create(PublicKeyCredential.prototype), {
      id: c.id,
      rawId: unb64(c.id),
      type: 'public-key',
      authenticatorAttachment: 'cross-platform',
      response: response,
      getClientExtensionResults: function() { return {}; }
    });
  }

  function showOverlay(qrSvg, onCancel) {
    var host = document.createElement('div');
    var root = host.attachShadow({ mode: 'closed' });
    root.innerHTML =
      '<style>' +
      '.back{position:fixed;inset:0;z-index:2147483647;background:rgba(32,33,36,.6);display:flex;align-items:center;justify-content:center;font:14px/20px "Google Sans",Roboto,Arial,sans-serif}' +
      '.card{background:#fff;color:#202124;border-radius:28px;padding:32px;width:320px;text-align:center;box-shadow:0 4px 24px rgba(0,0,0,.3)}' +
      'h2{font-size:22px;line-height:28px;font-weight:400;margin:0 0 8px}' +
      '.qr{width:220px;height:220px;margin:16px auto}.qr svg{width:100%;height:100%}' +
      '.status{color:#5f6368;min-height:40px}' +
      'button{margin-top:16px;border:0;background:none;color:#0b57d0;font:500 14px "Google Sans",Roboto,Arial,sans-serif;padding:10px 24px;border-radius:20px;cursor:pointer}' +
      'button:hover{background:#f0f4fc}' +
      '</style>' +
      '<div class="back"><div class="card">' +
      '<h2>Use a passkey from your phone</h2>' +
      '<div class="qr"></div>' +
      '<div class="status">Scan this code with your phone’s camera. Keep Bluetooth on for both devices.</div>' +
      '<button type="button">Cancel</button>' +
      '</div></div>';
    // The SVG comes from the app, which builds it from its own markup only.
    root.querySelector('.qr').innerHTML = qrSvg;
    root.querySelector('button').addEventListener('click', onCancel);
    document.documentElement.appendChild(host);
    return {
      status: function(text, hideQr) {
        root.querySelector('.status').textContent = text;
        if (hideQr) root.querySelector('.qr').style.display = 'none';
      },
      close: function() { host.remove(); }
    };
  }

  function hybridGet(options) {
    var pk = options.publicKey;
    var request = {
      challenge: b64(pk.challenge),
      rpId: pk.rpId || null,
      allowCredentials: (pk.allowCredentials || []).map(function(c) { return b64(c.id); }),
      timeout: pk.timeout || null
    };
    var signal = options.signal;
    if (signal && signal.aborted) {
      return Promise.reject(new DOMException('The operation was aborted.', 'AbortError'));
    }

    return invoke('passkey_start', { request: request }).then(function(started) {
      var session = started.session;
      var overlay = showOverlay(started.qrSvg, function() {
        invoke('passkey_cancel', { session: session });
      });
      var aborted = false;
      if (signal) {
        signal.addEventListener('abort', function() {
          aborted = true;
          invoke('passkey_cancel', { session: session });
        });
      }

      function poll() {
        return invoke('passkey_next', { session: session }).then(function(ev) {
          if (ev.state === 'pending') return poll();
          if (ev.state === 'connecting') {
            overlay.status('Connecting to your phone…', true);
            return poll();
          }
          if (ev.state === 'confirm') {
            overlay.status('Confirm the sign-in on your phone.', true);
            return poll();
          }
          overlay.close();
          if (ev.state === 'done') return toCredential(ev.credential);
          if (aborted) throw new DOMException('The operation was aborted.', 'AbortError');
          throw new DOMException(ev.message, ev.name);
        }, function(err) {
          overlay.close();
          throw err;
        });
      }
      return poll();
    }, function(err) {
      // The app refused the request (wrong origin, bad options).
      throw new DOMException(err && err.message || String(err), err && err.name || 'NotAllowedError');
    });
  }

  CredentialsContainer.prototype.get = function(options) {
    // Autofill ("conditional") requests stay native: they wait silently and
    // must not open a QR code by themselves.
    if (!options || !options.publicKey || options.mediation === 'conditional' ||
        !window.__TAURI_INTERNALS__) {
      return nativeGet.apply(this, arguments);
    }
    return hybridGet(options);
  };
})();
