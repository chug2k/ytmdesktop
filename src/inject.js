(function() {
  if (window.__ytm_injected) return;
  window.__ytm_injected = true;

  function extractTrackState() {
    const titleEl = document.querySelector('yt-formatted-string.title.ytmusic-player-bar');
    const bylineEl = document.querySelector('yt-formatted-string.byline.ytmusic-player-bar');
    const playPauseButton = document.querySelector('#play-pause-button');

    if (!titleEl || !bylineEl || !playPauseButton) {
      return null;
    }
    const title = titleEl.textContent;
    const artistLink = bylineEl.querySelector('a');
    const artist = artistLink ? artistLink.textContent : bylineEl.textContent.split(' \u2022 ')[0];
    const thumbnailEl = document.querySelector('img.ytmusic-player-bar')
      || document.querySelector('.ytmusic-player-bar img')
      || document.querySelector('#song-image img');
    const art = thumbnailEl ? thumbnailEl.src : '';
    const isPlaying = playPauseButton.getAttribute('title') === 'Pause';

    return { title, artist, art, isPlaying };
  }

  let lastStateJson = null;

  console.log('[YTM Yagami] inject.js loaded');

  setInterval(() => {
    const state = extractTrackState();
    if (!state) return;

    const json = JSON.stringify(state);
    if (json === lastStateJson) return;
    lastStateJson = json;

    console.log('[YTM Yagami] Track changed:', state.title, '-', state.artist);

    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
      window.__TAURI_INTERNALS__.invoke('handle_track_changed', { payload: state })
        .catch(function(err) { console.error('[YTM Yagami] invoke failed:', err); });
    }
  }, 1000);
})();
