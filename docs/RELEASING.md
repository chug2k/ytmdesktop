# Releasing

`.github/workflows/release.yml` builds and drafts a GitHub release on any `v*`
tag. It works today, but until macOS signing secrets exist it produces an
**unsigned** bundle that Gatekeeper blocks on anyone else's machine.

## Cutting a release

```sh
# bump the version in BOTH places — they are not linked
#   src-tauri/tauri.conf.json  ("version")
#   src-tauri/Cargo.toml       ([package] version)
git tag v0.1.0
git push origin v0.1.0
```

The workflow builds macOS (arm64 + x64), Linux, and Windows, then opens a
**draft** release. Review the assets and publish it manually.

## macOS signing and notarization — required before this is shippable

Without these, a downloader gets "YTM Yagami is damaged and can't be opened" or
has to right-click → Open, and notification permission may silently never be
granted, because `UNUserNotificationCenter` behaves differently for bundles that
are not properly signed.

You need an Apple Developer Program membership ($99/yr), then add these repo
secrets, which `tauri-action` already reads:

| Secret | What it is |
| --- | --- |
| `APPLE_CERTIFICATE` | base64 of your "Developer ID Application" `.p12` |
| `APPLE_CERTIFICATE_PASSWORD` | the `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | your Apple ID email |
| `APPLE_PASSWORD` | an app-specific password, not your Apple ID password |
| `APPLE_TEAM_ID` | your 10-character team ID |

Export the cert:

```sh
# from Keychain Access, export the Developer ID Application cert as cert.p12
base64 -i cert.p12 | pbcopy
```

You will also want hardened runtime enabled, which notarization requires. Add to
`src-tauri/tauri.conf.json`:

```json
"bundle": {
  "macOS": {
    "hardenedRuntime": true,
    "minimumSystemVersion": "11.0"
  }
}
```

Verify a built bundle before publishing:

```sh
codesign -dv --verbose=4 "src-tauri/target/release/bundle/macos/YTM Yagami.app"
spctl -a -vvv "src-tauri/target/release/bundle/macos/YTM Yagami.app"
```

## Still missing

- **No auto-updater.** There is no `tauri-plugin-updater` and no update
  endpoint, so every installed copy is frozen at whatever version it was. This
  matters more than usual here: the app reads YouTube Music's DOM, so a YTM
  redesign can break track detection and there is currently no way to ship the
  fix.
- **Windows and Linux notifications.** `src-tauri/src/macos_notifications.rs` is
  the only backend; the other platforms get media controls but no notifications.
- **Icon size.** `src-tauri/icons/icon.icns` is 1.5 MB and is a meaningful
  fraction of the 5.2 MB DMG.
