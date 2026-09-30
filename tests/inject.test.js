import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';

// Load and execute the *shipped* injector rather than a copy of it. The previous
// version of this file defined its own `extractTrackState`, which drifted out of
// sync with src/inject.js and ended up asserting the opposite behaviour.
//
// Resolved via strings, not `new URL(...)`: under the jsdom environment the
// global `URL` is jsdom's, and `readFileSync` rejects it as a non-file URL.
const HERE = dirname(fileURLToPath(import.meta.url));
const INJECT_SOURCE = readFileSync(resolve(HERE, '../src/inject.js'), 'utf8');

function loadInjector() {
  delete window.__ytm_injected;
  delete window.__ytmYagamiInternals;
  new Function(INJECT_SOURCE).call(window);
  return window.__ytmYagamiInternals;
}

/** jsdom's `paused` is a read-only getter, so state has to be stubbed on. */
function setPaused(paused) {
  const video = document.querySelector('video');
  Object.defineProperty(video, 'paused', { value: paused, configurable: true });
}

const PLAYER_BAR = `
  <ytmusic-player-bar>
    <yt-formatted-string class="title style-scope ytmusic-player-bar">Never Gonna Give You Up</yt-formatted-string>
    <span class="subtitle style-scope ytmusic-player-bar">
      <yt-formatted-string class="byline style-scope ytmusic-player-bar">
        <a href="/channel/UC38IQsAvIsxxjztdMZQvwEA">Rick Astley</a> • <a href="/browse/MPREb_k0xWqKkxA7U">Whenever You Need Somebody</a> • <span>1987</span>
      </yt-formatted-string>
    </span>
    <img id="thumbnail" class="style-scope ytmusic-player-bar" src="https://lh3.googleusercontent.com/art?w=544">
    <tp-yt-paper-icon-button id="play-pause-button" title="Pause"></tp-yt-paper-icon-button>
  </ytmusic-player-bar>
  <video></video>
`;

let internals;

afterEach(() => {
  // The injector installs a MutationObserver and possibly a 1s poll.
  internals?.stop();
  internals = undefined;
  document.body.innerHTML = '';
});

describe('extractTrackState', () => {
  it('extracts title, artist, and album art from the player bar', () => {
    document.body.innerHTML = PLAYER_BAR;
    internals = loadInjector();
    setPaused(false);

    expect(internals.extractTrackState(document)).toEqual({
      title: 'Never Gonna Give You Up',
      artist: 'Rick Astley',
      art: 'https://lh3.googleusercontent.com/art?w=544',
      isPlaying: true
    });
  });

  it('reports paused when the video element is paused', () => {
    document.body.innerHTML = PLAYER_BAR;
    internals = loadInjector();
    setPaused(true);

    expect(internals.extractTrackState(document).isPlaying).toBe(false);
  });

  it('reports playing on a non-English UI, where the button title is localized', () => {
    // Regression test: reading `title === 'Pause'` reported permanently-paused
    // on localized UIs, which silently disabled notifications entirely.
    document.body.innerHTML = PLAYER_BAR.replace('title="Pause"', 'title="Anhalten"');
    internals = loadInjector();
    setPaused(false);

    expect(internals.extractTrackState(document).isPlaying).toBe(true);
  });

  it('returns empty art rather than null when no thumbnail is present', () => {
    document.body.innerHTML = PLAYER_BAR.replace(/<img[^>]*>/, '');
    internals = loadInjector();
    setPaused(false);

    const state = internals.extractTrackState(document);
    expect(state).not.toBeNull();
    expect(state.art).toBe('');
  });

  it('falls back to byline text when no artist link exists', () => {
    document.body.innerHTML = `
      <ytmusic-player-bar>
        <yt-formatted-string class="title style-scope ytmusic-player-bar">Unknown Track</yt-formatted-string>
        <yt-formatted-string class="byline style-scope ytmusic-player-bar">Various Artists • Compilation • 2024</yt-formatted-string>
      </ytmusic-player-bar>
      <video></video>
    `;
    internals = loadInjector();
    setPaused(false);

    expect(internals.extractTrackState(document).artist).toBe('Various Artists');
  });

  it('returns null when the player bar has not rendered yet', () => {
    document.body.innerHTML = `<div>Not the player</div>`;
    internals = loadInjector();

    expect(internals.extractTrackState(document)).toBeNull();
  });

  it('returns null when the title is missing', () => {
    document.body.innerHTML = `
      <ytmusic-player-bar>
        <yt-formatted-string class="byline style-scope ytmusic-player-bar"><a href="#">Artist</a></yt-formatted-string>
      </ytmusic-player-bar>
      <video></video>
    `;
    internals = loadInjector();

    expect(internals.extractTrackState(document)).toBeNull();
  });
});

describe('extractIsPlaying', () => {
  it('prefers the video element over the localized button title', () => {
    document.body.innerHTML = PLAYER_BAR.replace('title="Pause"', 'title="Play"');
    internals = loadInjector();
    setPaused(false);

    expect(internals.extractIsPlaying(document)).toBe(true);
  });

  it('falls back to the button title when there is no video element', () => {
    document.body.innerHTML = PLAYER_BAR.replace('<video></video>', '');
    internals = loadInjector();

    expect(internals.extractIsPlaying(document)).toBe(true);
  });

  it('returns null when neither a video nor a play/pause button exists', () => {
    document.body.innerHTML = `<div></div>`;
    internals = loadInjector();

    expect(internals.extractIsPlaying(document)).toBeNull();
  });
});
