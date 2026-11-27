//! Cookies for yt-dlp from a browser's profile (`--cookies-from-browser`), e.g. for age-restricted
//! or members-only videos.

use serde::{Deserialize, Deserializer, Serialize};

/// The Linux browsers yt-dlp reads cookies from; serialized as yt-dlp names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Browser {
    Brave,
    Chrome,
    Chromium,
    Edge,
    Firefox,
    Opera,
    Vivaldi,
    Whale,
}

impl Browser {
    pub const ALL: [Self; 8] = [
        Self::Brave,
        Self::Chrome,
        Self::Chromium,
        Self::Edge,
        Self::Firefox,
        Self::Opera,
        Self::Vivaldi,
        Self::Whale,
    ];

    /// yt-dlp's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Brave => "brave",
            Self::Chrome => "chrome",
            Self::Chromium => "chromium",
            Self::Edge => "edge",
            Self::Firefox => "firefox",
            Self::Opera => "opera",
            Self::Vivaldi => "vivaldi",
            Self::Whale => "whale",
        }
    }

    /// The name to show.
    pub fn label(self) -> &'static str {
        match self {
            Self::Brave => "Brave",
            Self::Chrome => "Chrome",
            Self::Chromium => "Chromium",
            Self::Edge => "Edge",
            Self::Firefox => "Firefox",
            Self::Opera => "Opera",
            Self::Vivaldi => "Vivaldi",
            Self::Whale => "Whale",
        }
    }
}

/// Which browser profile yt-dlp takes cookies from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserCookies {
    pub browser: Browser,
    /// A profile name or path; `None` lets yt-dlp take the most recently used profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
}

impl BrowserCookies {
    /// A blank `profile` means none.
    pub fn new(browser: Browser, profile: Option<String>) -> Self {
        let profile = profile.filter(|profile| !profile.trim().is_empty());
        Self { browser, profile }
    }

    /// The value of yt-dlp's `--cookies-from-browser`: `firefox` or `firefox:<profile>`.
    pub fn argument(&self) -> String {
        match &self.profile {
            Some(profile) => format!("{}:{}", self.browser.name(), profile.trim()),
            None => self.browser.name().to_owned(),
        }
    }
}

/// Reads the setting, dropping it (with a warning) instead of rejecting the file when its browser
/// is unknown.
pub fn deserialize_lenient<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<BrowserCookies>, D::Error> {
    #[derive(Deserialize)]
    struct Raw {
        browser: String,
        profile: Option<String>,
    }
    let Some(raw) = Option::<Raw>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let Some(browser) = Browser::ALL.into_iter().find(|b| b.name() == raw.browser) else {
        tracing::warn!(
            "ignoring cookies_from_browser in the config: unknown browser {:?}",
            raw.browser
        );
        return Ok(None);
    };
    Ok(Some(BrowserCookies::new(browser, raw.profile)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct Wrapper {
        #[serde(default, deserialize_with = "deserialize_lenient")]
        cookies: Option<BrowserCookies>,
    }

    fn parse(text: &str) -> Option<BrowserCookies> {
        toml::from_str::<Wrapper>(text).expect("valid toml").cookies
    }

    #[test]
    fn every_browser_is_written_with_its_yt_dlp_name_and_read_back() {
        for browser in Browser::ALL {
            let cookies = Some(BrowserCookies::new(browser, None));
            let text = toml::to_string(&Wrapper {
                cookies: cookies.clone(),
            })
            .expect("serialize");
            assert!(text.contains(&format!("\"{}\"", browser.name())), "{text}");
            assert_eq!(parse(&text), cookies);
        }
    }

    #[test]
    fn blank_profiles_and_unknown_browsers_are_dropped() {
        let firefox = |profile: Option<&str>| {
            Some(BrowserCookies {
                browser: Browser::Firefox,
                profile: profile.map(str::to_owned),
            })
        };
        let text = "cookies = { browser = \"firefox\", profile = \"~/.librewolf/x\" }";
        assert_eq!(parse(text), firefox(Some("~/.librewolf/x")));
        let blank = "cookies = { browser = \"firefox\", profile = \"  \" }";
        assert_eq!(parse(blank), firefox(None));
        assert_eq!(parse("cookies = { browser = \"netscape\" }"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn the_argument_names_the_browser_and_profile() {
        assert_eq!(
            BrowserCookies::new(Browser::Chrome, None).argument(),
            "chrome"
        );
        let profile = Some(" Profile 1 ".to_owned());
        assert_eq!(
            BrowserCookies::new(Browser::Chrome, profile).argument(),
            "chrome:Profile 1"
        );
    }
}
