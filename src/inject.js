(function() {
  if (window.__ytm_injected) return;
  window.__ytm_injected = true;

  function extractTrackState(doc) {
    doc = doc || document;

    var titleEl = doc.querySelector('yt-formatted-string.title.ytmusic-player-bar');
    var bylineEl = doc.querySelector('yt-formatted-string.byline.ytmusic-player-bar');
    if (!titleEl || !bylineEl) return null;

    var isPlaying = extractIsPlaying(doc);
    if (isPlaying === null) return null;

    var title = titleEl.textContent;
    var artistLink = bylineEl.querySelector('a');
    var artist = artistLink ? artistLink.textContent : bylineEl.textContent.split(' • ')[0];
    var thumbnailEl = doc.querySelector('img.ytmusic-player-bar')
      || doc.querySelector('.ytmusic-player-bar img')
      || doc.querySelector('#song-image img');
    var art = thumbnailEl ? thumbnailEl.src : '';

    return { title: title, artist: artist, art: art, isPlaying: isPlaying };
  }

  // The <video> element is the only locale-independent source of truth here.
  // The play/pause button's `title` is localized ("Pause" / "Anhalten" / "暂停"),
  // so comparing it against the English string reports permanently-paused on any
  // non-English UI — which silently disables notifications entirely.
  function extractIsPlaying(doc) {
    var video = doc.querySelector('video');
    if (video) return !video.paused;

    var playPauseButton = doc.querySelector('#play-pause-button');
    if (!playPauseButton) return null;
    return playPauseButton.getAttribute('title') === 'Pause';
  }

  var lastStateJson = null;

  function sendIfChanged() {
    var state = extractTrackState(document);
    if (!state) return;

    var json = JSON.stringify(state);
    if (json === lastStateJson) return;
    lastStateJson = json;

    if (window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke) {
      window.__TAURI_INTERNALS__.invoke('handle_track_changed', { payload: state })
        .catch(function(err) {
          // A silent failure here means notifications and OS media controls are
          // dead with nothing to show for it. Make it visible in the console.
          console.error('[YTM Yagami] IPC invoke failed:', err);
        });
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

  var observer = null;
  var pollId = null;

  function observePlayerBar() {
    var playerBar = document.querySelector('ytmusic-player-bar');
    if (!playerBar) return false;

    observer = new MutationObserver(scheduleCheck);
    observer.observe(playerBar, { childList: true, subtree: true, attributes: true, characterData: true });

    // <video> state changes without mutating the player bar's DOM.
    var video = document.querySelector('video');
    if (video) {
      video.addEventListener('play', scheduleCheck);
      video.addEventListener('pause', scheduleCheck);
      video.addEventListener('loadedmetadata', scheduleCheck);
    }
    return true;
  }

  function stop() {
    if (observer) { observer.disconnect(); observer = null; }
    if (pollId !== null) { clearInterval(pollId); pollId = null; }
  }

  if (!observePlayerBar()) {
    pollId = setInterval(function() {
      if (observePlayerBar()) {
        clearInterval(pollId);
        pollId = null;
        sendIfChanged();
      }
    }, 1000);
  }

  // Exposed so tests exercise the shipped extractor rather than a copy of it.
  window.__ytmYagamiInternals = {
    extractTrackState: extractTrackState,
    extractIsPlaying: extractIsPlaying,
    stop: stop
  };
})();
