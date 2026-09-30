// @vitest-environment-options {"url": "https://accounts.google.com/v3/signin/challenge/pk"}
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { beforeEach, describe, expect, it, vi } from 'vitest';

// Execute the shipped shim, as tests/inject.test.js does for the injector.
const HERE = dirname(fileURLToPath(import.meta.url));
const SOURCE = readFileSync(resolve(HERE, '../src/passkey_hybrid.js'), 'utf8');

const b64 = (bytes) =>
  btoa(String.fromCharCode(...bytes)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

let nativeGet;
let calls;

// jsdom has no WebAuthn; stand in the classes the shim builds on.
function installWebAuthnStubs() {
  nativeGet = vi.fn(() => Promise.resolve('native'));
  window.CredentialsContainer = function CredentialsContainer() {};
  window.CredentialsContainer.prototype.get = nativeGet;
  window.PublicKeyCredential = function PublicKeyCredential() {};
  window.AuthenticatorAssertionResponse = function AuthenticatorAssertionResponse() {};
}

function installIpc(events) {
  calls = [];
  window.__TAURI_INTERNALS__ = {
    invoke: vi.fn((cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === 'passkey_start') return Promise.resolve({ session: 7, qrSvg: '<svg id="qr"></svg>' });
      if (cmd === 'passkey_next') return Promise.resolve(events.shift());
      return Promise.resolve(null);
    }),
  };
}

function load() {
  delete window.__ytm_passkey_applied;
  new Function(SOURCE).call(window);
  return Object.create(window.CredentialsContainer.prototype);
}

const CREDENTIAL = {
  id: b64([1, 2, 3, 4]),
  clientDataJson: b64([123, 125]),
  authenticatorData: b64([9, 9]),
  signature: b64([8, 8, 8]),
  userHandle: b64([5]),
};

describe('passkey hybrid shim', () => {
  beforeEach(() => {
    installWebAuthnStubs();
    document.documentElement.innerHTML = '<head></head><body></body>';
  });

  it('sends the request to the app and returns a PublicKeyCredential', async () => {
    installIpc([{ state: 'pending' }, { state: 'connecting' }, { state: 'confirm' }, { state: 'done', credential: CREDENTIAL }]);
    const credentials = load();

    const cred = await credentials.get({
      publicKey: {
        challenge: new Uint8Array([0xfb, 0xff, 0x00]),
        rpId: 'google.com',
        allowCredentials: [{ type: 'public-key', id: new Uint8Array([1, 2, 3, 4]).buffer }],
        timeout: 60000,
      },
    });

    expect(calls[0]).toEqual(['passkey_start', {
      request: { challenge: '-_8A', rpId: 'google.com', allowCredentials: ['AQIDBA'], timeout: 60000 },
    }]);
    expect(cred).toBeInstanceOf(window.PublicKeyCredential);
    expect(cred.id).toBe(CREDENTIAL.id);
    expect(cred.type).toBe('public-key');
    expect([...new Uint8Array(cred.rawId)]).toEqual([1, 2, 3, 4]);
    expect(cred.response).toBeInstanceOf(window.AuthenticatorAssertionResponse);
    expect([...new Uint8Array(cred.response.signature)]).toEqual([8, 8, 8]);
    expect([...new Uint8Array(cred.response.userHandle)]).toEqual([5]);
    expect(cred.getClientExtensionResults()).toEqual({});
    expect(nativeGet).not.toHaveBeenCalled();
  });

  it('turns a failure into the DOMException the app named', async () => {
    installIpc([{ state: 'failed', name: 'NotAllowedError', message: 'Bluetooth is off' }]);
    const credentials = load();
    const result = credentials.get({ publicKey: { challenge: new Uint8Array(32) } });
    await expect(result).rejects.toMatchObject({ name: 'NotAllowedError', message: 'Bluetooth is off' });
  });

  it('leaves autofill requests and non-WebAuthn requests to the engine', async () => {
    installIpc([]);
    const credentials = load();
    await credentials.get({ publicKey: { challenge: new Uint8Array(32) }, mediation: 'conditional' });
    await credentials.get({ password: true });
    expect(nativeGet).toHaveBeenCalledTimes(2);
    expect(calls).toEqual([]);
  });

  it('cancels the session when the page aborts', async () => {
    let release;
    installIpc([]);
    window.__TAURI_INTERNALS__.invoke = vi.fn((cmd, args) => {
      calls.push([cmd, args]);
      if (cmd === 'passkey_start') return Promise.resolve({ session: 3, qrSvg: '<svg></svg>' });
      if (cmd === 'passkey_next') return new Promise((r) => { release = r; });
      return Promise.resolve(null);
    });
    const credentials = load();
    const controller = new AbortController();
    const result = credentials.get({ publicKey: { challenge: new Uint8Array(32) }, signal: controller.signal });
    await vi.waitFor(() => expect(release).toBeTypeOf('function'));
    controller.abort();
    release({ state: 'failed', name: 'NotAllowedError', message: 'Cancelled' });
    await expect(result).rejects.toMatchObject({ name: 'AbortError' });
    expect(calls).toContainEqual(['passkey_cancel', { session: 3 }]);
  });
});
