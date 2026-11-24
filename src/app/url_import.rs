//! The "Add sound from URL" dialog's state: what to download into which tab, and how it goes.
//! Closing the dialog cancels a running download.

use std::path::PathBuf;

use crate::config::TabConfig;
use crate::import::Stage;

#[derive(Debug, Clone, PartialEq)]
pub struct UrlImport {
    pub url: String,
    /// The tab to save into (an index into `config.tabs`).
    pub tab: usize,
    pub status: ImportStatus,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportStatus {
    /// Waiting for the user to start the download.
    Idle,
    Running(Stage),
    /// Why the last attempt failed; the user may retry.
    Failed(String),
}

/// Something the user did in the dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum UrlImportAction {
    Url(String),
    Tab(usize),
    /// Download, or retry after a failure.
    Download,
    /// Close the dialog, cancelling a running download.
    Close,
}

impl UrlImport {
    /// The dialog for saving into `tab`.
    pub fn open(tab: usize) -> Self {
        Self {
            url: String::new(),
            tab,
            status: ImportStatus::Idle,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, ImportStatus::Running(_))
    }

    /// Edits the URL or the tab; ignored while downloading.
    pub fn edit(&mut self, action: UrlImportAction) {
        if self.is_running() {
            return;
        }
        match action {
            UrlImportAction::Url(url) => self.url = url,
            UrlImportAction::Tab(tab) => self.tab = tab,
            UrlImportAction::Download | UrlImportAction::Close => {}
        }
    }

    pub fn can_download(&self, tabs: &[TabConfig]) -> bool {
        !self.is_running() && check_url(&self.url).is_ok() && self.tab < tabs.len()
    }

    /// Marks the download as started and returns what to download into which folder, if it can
    /// start.
    pub fn start(&mut self, tabs: &[TabConfig]) -> Option<(String, PathBuf)> {
        if !self.can_download(tabs) {
            return None;
        }
        self.status = ImportStatus::Running(Stage::Starting);
        Some((self.url.trim().to_owned(), tabs[self.tab].path.clone()))
    }
}

/// Whether `url` looks like something yt-dlp can fetch: an http(s) URL with a host.
pub fn check_url(url: &str) -> Result<(), &'static str> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let rest = ["https://", "http://"]
        .iter()
        .find_map(|scheme| lower.strip_prefix(scheme))
        .ok_or("Enter a URL starting with http:// or https://")?;
    if rest.contains(char::is_whitespace) {
        return Err("A URL can't contain spaces");
    }
    if rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .is_empty()
    {
        return Err("The URL has no host");
    }
    Ok(())
}

/// The progress bar's text.
pub fn stage_text(stage: Stage) -> String {
    match stage {
        Stage::Starting => "Starting…".to_owned(),
        Stage::Downloading(Some(fraction)) => format!("Downloading… {:.0}%", fraction * 100.0),
        Stage::Downloading(None) => "Downloading…".to_owned(),
        Stage::Converting => "Converting to MP3…".to_owned(),
    }
}

/// How full the progress bar is; `None` while that's unknown (animated instead).
pub fn stage_fraction(stage: Stage) -> Option<f32> {
    match stage {
        Stage::Downloading(fraction) => fraction,
        Stage::Starting | Stage::Converting => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs() -> Vec<TabConfig> {
        vec![
            TabConfig::for_folder("/sounds/Memes".into()),
            TabConfig::for_folder("/sounds/SFX".into()),
        ]
    }

    #[test]
    fn only_http_urls_with_a_host_are_accepted() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "http://127.0.0.1:8000/tone.wav",
            "  HTTPS://example.com  ",
        ] {
            assert_eq!(check_url(url), Ok(()), "{url}");
        }
        for url in [
            "",
            "youtube.com/watch?v=1",
            "ftp://example.com/a.mp3",
            "file:///etc/passwd",
            "https://",
            "https:///path",
            "https://exa mple.com",
            "-https://example.com",
        ] {
            assert!(check_url(url).is_err(), "{url}");
        }
    }

    #[test]
    fn starting_needs_a_valid_url_and_tab() {
        let tabs = tabs();
        let mut import = UrlImport::open(1);
        assert_eq!(import.start(&tabs), None);
        assert_eq!(import.status, ImportStatus::Idle);

        import.edit(UrlImportAction::Url(" https://example.com/a ".to_owned()));
        assert_eq!(
            import.start(&tabs),
            Some(("https://example.com/a".to_owned(), "/sounds/SFX".into()))
        );
        assert_eq!(import.status, ImportStatus::Running(Stage::Starting));

        let mut import = UrlImport::open(0);
        import.edit(UrlImportAction::Url("https://example.com/a".to_owned()));
        assert_eq!(import.start(&[]), None, "no tabs");
    }

    #[test]
    fn nothing_changes_while_downloading() {
        let tabs = tabs();
        let mut import = UrlImport::open(0);
        import.edit(UrlImportAction::Url("https://example.com/a".to_owned()));
        import.edit(UrlImportAction::Tab(1));
        assert!(import.start(&tabs).is_some());

        import.edit(UrlImportAction::Url("https://other.example".to_owned()));
        import.edit(UrlImportAction::Tab(0));
        assert_eq!(import.url, "https://example.com/a");
        assert_eq!(import.tab, 1);
        assert!(!import.can_download(&tabs));
        assert_eq!(import.start(&tabs), None);
    }

    #[test]
    fn a_failed_download_can_be_retried() {
        let tabs = tabs();
        let mut import = UrlImport::open(0);
        import.edit(UrlImportAction::Url("https://example.com/a".to_owned()));
        import.status = ImportStatus::Failed("HTTP Error 404".to_owned());
        assert!(import.can_download(&tabs));
        assert!(import.start(&tabs).is_some());
    }

    #[test]
    fn stages_are_described() {
        assert_eq!(stage_text(Stage::Starting), "Starting…");
        assert_eq!(
            stage_text(Stage::Downloading(Some(0.426))),
            "Downloading… 43%"
        );
        assert_eq!(stage_text(Stage::Downloading(None)), "Downloading…");
        assert_eq!(stage_text(Stage::Converting), "Converting to MP3…");
        assert_eq!(stage_fraction(Stage::Downloading(Some(0.5))), Some(0.5));
        assert_eq!(stage_fraction(Stage::Converting), None);
    }
}
