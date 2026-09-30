import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Execute the shipped guard, as tests/inject.test.js does for the injector.
const HERE = dirname(fileURLToPath(import.meta.url));
const GUARD_SOURCE = readFileSync(resolve(HERE, '../src/ipc_transport.js'), 'utf8');

describe('ipc:// fetch guard', () => {
  let realFetch;
  let pageFetch;

  beforeEach(() => {
    realFetch = window.fetch;
    pageFetch = vi.fn(() => Promise.resolve('network'));
    window.fetch = pageFetch;
    delete window.__ytm_ipc_guard_applied;
    new Function(GUARD_SOURCE).call(window);
  });

  afterEach(() => {
    window.fetch = realFetch;
  });

  it('rejects string ipc:// URLs without touching the network', async () => {
    await expect(window.fetch('ipc://localhost/handle_track_changed')).rejects.toThrow(TypeError);
    expect(pageFetch).not.toHaveBeenCalled();
  });

  it('passes ordinary URLs through with their arguments', async () => {
    const init = { method: 'POST' };
    await expect(window.fetch('https://music.youtube.com/youtubei/v1/next', init)).resolves.toBe('network');
    expect(pageFetch).toHaveBeenCalledWith('https://music.youtube.com/youtubei/v1/next', init);
  });

  it('installs once when evaluated twice', () => {
    const guarded = window.fetch;
    new Function(GUARD_SOURCE).call(window);
    expect(window.fetch).toBe(guarded);
  });
});
