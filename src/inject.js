(function() {
  if (window.__ytm_injected) return;
  window.__ytm_injected = true;

  function extractTrackState() {
    var titleEl = document.querySelector('yt-formatted-string.title.ytmusic-player-bar');
    var bylineEl = document.querySelector('yt-formatted-string.byline.ytmusic-player-bar');
    var playPauseButton = document.querySelector('#play-pause-button');
    if (!titleEl || !bylineEl || !playPauseButton) return null;

    var title = titleEl.textContent;
    var artistLink = bylineEl.querySelector('a');
    var artist = artistLink ? artistLink.textContent : bylineEl.textContent.split(' \u2022 ')[0];
    var thumbnailEl = document.querySelector('img.ytmusic-player-bar')
      || document.querySelector('.ytmusic-player-bar img')
      || document.querySelector('#song-image img');
    var art = thumbnailEl ? thumbnailEl.src : '';
    var isPlaying = playPauseButton.getAttribute('title') === 'Pause';

    return { title: title, artist: artist, art: art, isPlaying: isPlaying };
  }

  var lastStateJson = null;

  function sendIfChanged() {
    var state = extractTrackState();
    if (!state) return;

    var json = JSON.stringify(state);
    if (json === lastStateJson) return;
    lastStateJson = json;

    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
      window.__TAURI_INTERNALS__.invoke('handle_track_changed', { payload: state })
        .catch(function() {});
    }
  }

  // Use MutationObserver on the player bar for event-driven updates
  function observePlayerBar() {
    var playerBar = document.querySelector('ytmusic-player-bar');
    if (!playerBar) return false;

    var observer = new MutationObserver(sendIfChanged);
    observer.observe(playerBar, { childList: true, subtree: true, attributes: true, characterData: true });
    return true;
  }

  // Try to attach observer, fall back to polling until player bar exists
  if (!observePlayerBar()) {
    var pollId = setInterval(function() {
      if (observePlayerBar()) {
        clearInterval(pollId);
        sendIfChanged();
      }
    }, 1000);
  }

  // Also check on play/pause which may not trigger mutation
  setInterval(sendIfChanged, 2000);
})();
