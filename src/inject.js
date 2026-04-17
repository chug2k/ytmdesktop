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

  // Player bar mutates many times per second (progress bar, time text);
  // coalesce bursts into one check per frame.
  var rafScheduled = false;
  function scheduleCheck() {
    if (rafScheduled) return;
    rafScheduled = true;
    requestAnimationFrame(function() {
      rafScheduled = false;
      sendIfChanged();
    });
  }

  function observePlayerBar() {
    var playerBar = document.querySelector('ytmusic-player-bar');
    if (!playerBar) return false;

    var observer = new MutationObserver(scheduleCheck);
    observer.observe(playerBar, { childList: true, subtree: true, attributes: true, characterData: true });
    return true;
  }

  if (!observePlayerBar()) {
    var pollId = setInterval(function() {
      if (observePlayerBar()) {
        clearInterval(pollId);
        sendIfChanged();
      }
    }, 1000);
  }
})();
