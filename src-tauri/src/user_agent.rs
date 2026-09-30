//! The user agent the macOS webview presents. The whole module is gated behind
//! `#[cfg(target_os = "macos")]` at the `mod` declaration in lib.rs.
//!
//! Google refuses sign-in from a bare WKWebView ("This browser or app may not
//! be secure") because its default UA stops after `(KHTML, like Gecko)`, which
//! marks it as an embedded webview. Adding Safari's `Version/… Safari/…` tokens
//! makes the UA identical to Safari's, and Safari is what the engine is. The
//! earlier Chrome UA contradicted the engine underneath, and Google's scripts
//! can detect that contradiction.
//!
//! Other platforms keep their engine's own UA: WebView2 already reports a
//! current Edge, and WebKitGTK already includes the Safari tokens.

/// Safari's UA is frozen except for `Version/`: the OS token stays at
/// `10_15_7` and both WebKit tokens stay at `605.1.15` on every release.
const SAFARI_INFO_PLIST: &str = "/Applications/Safari.app/Contents/Info.plist";
/// Used only if the installed Safari's version cannot be read.
const FALLBACK_SAFARI_VERSION: &str = "26.0";

/// A Safari UA carrying the installed Safari's version, so it never goes stale
/// the way a pinned browser version does.
pub fn safari() -> String {
    let version = std::fs::read_to_string(SAFARI_INFO_PLIST)
        .ok()
        .and_then(|plist| bundle_short_version(&plist))
        .unwrap_or_else(|| FALLBACK_SAFARI_VERSION.to_string());
    safari_with_version(&version)
}

fn safari_with_version(version: &str) -> String {
    format!(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
         (KHTML, like Gecko) Version/{version} Safari/605.1.15"
    )
}

/// Read `CFBundleShortVersionString` from an XML plist. Anything other than a
/// short dotted number gives `None`, so a damaged file cannot put arbitrary
/// text into the header of every request.
fn bundle_short_version(plist: &str) -> Option<String> {
    let (_, after_key) = plist.split_once("<key>CFBundleShortVersionString</key>")?;
    let (value, _) = after_key
        .trim_start()
        .strip_prefix("<string>")?
        .split_once("</string>")?;
    let value = value.trim();
    let is_version = value.len() <= 16
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    is_version.then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Safari</string>
	<key>CFBundleShortVersionString</key>
	<string>18.6</string>
	<key>CFBundleVersion</key>
	<string>20621.3.11.11.3</string>
</dict>
</plist>"#;

    #[test]
    fn reads_the_short_version_not_the_build_number() {
        assert_eq!(bundle_short_version(PLIST).as_deref(), Some("18.6"));
    }

    #[test]
    fn accepts_patch_versions() {
        let plist = PLIST.replace("18.6", "17.4.1");
        assert_eq!(bundle_short_version(&plist).as_deref(), Some("17.4.1"));
    }

    #[test]
    fn rejects_values_that_are_not_versions() {
        for bad in [
            "",
            "18..6",
            "18.6 beta",
            "18.6\r\nX-Evil: 1",
            "1.2.3.4.5.6.7.8.9",
        ] {
            let plist = PLIST.replace("18.6", bad);
            assert_eq!(bundle_short_version(&plist), None, "accepted {bad:?}");
        }
    }

    #[test]
    fn missing_key_gives_none() {
        assert_eq!(bundle_short_version("<plist><dict></dict></plist>"), None);
        assert_eq!(bundle_short_version(""), None);
    }

    #[test]
    fn user_agent_matches_real_safari_format() {
        // The format Safari itself sends.
        assert_eq!(
            safari_with_version("18.6"),
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 \
             (KHTML, like Gecko) Version/18.6 Safari/605.1.15"
        );
    }

    #[test]
    fn user_agent_never_claims_chrome() {
        let ua = safari();
        assert!(!ua.contains("Chrome"), "{ua}");
        assert!(
            ua.contains(" Version/") && ua.ends_with(" Safari/605.1.15"),
            "{ua}"
        );
    }
}
