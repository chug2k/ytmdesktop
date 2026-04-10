import { describe, it, expect } from 'vitest';

function extractTrackState(doc) {
  const titleEl = doc.querySelector('yt-formatted-string.title.ytmusic-player-bar');
  const bylineEl = doc.querySelector('yt-formatted-string.byline.ytmusic-player-bar');
  const thumbnailEl = doc.querySelector('img#thumbnail.ytmusic-player-bar');
  const playPauseButton = doc.querySelector('#play-pause-button');

  if (!titleEl || !bylineEl || !thumbnailEl || !playPauseButton) {
    return null;
  }
  const title = titleEl.textContent;
  const artistLink = bylineEl.querySelector('a');
  const artist = artistLink ? artistLink.textContent : bylineEl.textContent.split(' • ')[0];
  const art = thumbnailEl.src;
  const isPlaying = playPauseButton.getAttribute('title') === 'Pause';

  return { title, artist, art, isPlaying };
}

describe('YouTube Music DOM Extractor', () => {
  it('extracts track title, artist, and album art from the DOM', () => {
    // Mock the DOM elements that YouTube Music uses
    document.body.innerHTML = `
      <yt-formatted-string class="title style-scope ytmusic-player-bar">Never Gonna Give You Up</yt-formatted-string>
      <span class="subtitle style-scope ytmusic-player-bar">
        <yt-formatted-string class="byline style-scope ytmusic-player-bar">
          <a href="/channel/UC38IQsAvIsxxjztdMZQvwEA">Rick Astley</a> • <a href="/browse/MPREb_k0xWqKkxA7U">Whenever You Need Somebody</a> • <span>1987</span>
        </yt-formatted-string>
      </span>
      <img id="thumbnail" class="style-scope ytmusic-player-bar" src="https://lh3.googleusercontent.com/art_url=w544-h544-l90-rj">
      <tp-yt-paper-icon-button id="play-pause-button" title="Pause"></tp-yt-paper-icon-button>
    `;

    const state = extractTrackState(document);

    expect(state).toEqual({
      title: 'Never Gonna Give You Up',
      artist: 'Rick Astley',
      art: 'https://lh3.googleusercontent.com/art_url=w544-h544-l90-rj',
      isPlaying: true
    });
  });

  it('handles paused state correctly', () => {
    document.body.innerHTML = `
      <yt-formatted-string class="title style-scope ytmusic-player-bar">Song Name</yt-formatted-string>
      <span class="subtitle style-scope ytmusic-player-bar">
        <yt-formatted-string class="byline style-scope ytmusic-player-bar">
          <a href="#">Artist Name</a>
        </yt-formatted-string>
      </span>
      <img id="thumbnail" class="style-scope ytmusic-player-bar" src="img.jpg">
      <tp-yt-paper-icon-button id="play-pause-button" title="Play"></tp-yt-paper-icon-button>
    `;

    const state = extractTrackState(document);
    expect(state.isPlaying).toBe(false);
  });

  it('returns null if elements are missing', () => {
    document.body.innerHTML = `<div>Not the player</div>`;
    const state = extractTrackState(document);
    expect(state).toBeNull();
  });
});
