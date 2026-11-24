//! Adding sounds from URLs: a [`Downloader`] fetches the audio of a web page (e.g. a YouTube
//! video) as MP3 into a tab's folder. [`ytdlp::YtDlp`] does it with `yt-dlp` and `ffmpeg`.

pub mod ytdlp;

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// How far a running download is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Stage {
    /// Looking at the page, before any audio arrives.
    Starting,
    /// Downloading; the fraction done, if the size is known.
    Downloading(Option<f32>),
    /// Converting the download to MP3.
    Converting,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DownloadEvent {
    Progress(Stage),
    /// The sound file is in the folder now.
    Done(PathBuf),
    /// Why the download failed, for the user.
    Failed(String),
}

/// Downloads the audio of one URL at a time. The app polls it once per frame.
pub trait Downloader {
    /// Why downloads can't work (e.g. missing tools), as checked at startup.
    fn unavailable(&self) -> Option<String>;
    /// Starts downloading the audio of `url` as MP3 into `folder`, cancelling a running download.
    /// Partial files never appear in `folder`.
    fn start(&mut self, url: &str, folder: &Path);
    /// Stops the running download, if any, and removes its partial files.
    fn cancel(&mut self);
    /// The events since the last call.
    fn poll(&mut self) -> Vec<DownloadEvent>;
}

/// The programs a download runs (`ffprobe` comes with `ffmpeg`).
pub const TOOLS: &[&str] = &["yt-dlp", "ffmpeg", "ffprobe"];

/// The `tools` that aren't executable files in any directory of `path_var` (like `$PATH`).
pub fn missing_tools<'a>(tools: &[&'a str], path_var: Option<&OsStr>) -> Vec<&'a str> {
    let dirs: Vec<PathBuf> = path_var
        .map(std::env::split_paths)
        .into_iter()
        .flatten()
        .collect();
    tools
        .iter()
        .copied()
        .filter(|tool| !dirs.iter().any(|dir| is_executable(&dir.join(tool))))
        .collect()
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Explains what to install when `missing` isn't empty.
pub fn unavailable_message(missing: &[&str]) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "Adding sounds from URLs needs yt-dlp and ffmpeg; install them with your package \
         manager (not found: {}).",
        missing.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn create(dir: &Path, name: &str, mode: u32) {
        let path = dir.join(name);
        fs::write(&path, b"#!/bin/sh\n").expect("write");
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).expect("chmod");
    }

    #[test]
    fn tools_are_found_as_executables_on_the_path() {
        let a = tempfile::tempdir().expect("temp dir");
        let b = tempfile::tempdir().expect("temp dir");
        create(a.path(), "yt-dlp", 0o755);
        create(b.path(), "ffmpeg", 0o755);
        create(b.path(), "ffprobe", 0o644);
        fs::create_dir(a.path().join("ffprobe")).expect("mkdir");
        let path = std::env::join_paths([a.path(), b.path()]).expect("join");

        assert_eq!(missing_tools(TOOLS, Some(&path)), ["ffprobe"]);
        assert_eq!(missing_tools(TOOLS, None), TOOLS);
        assert_eq!(missing_tools(TOOLS, Some(OsStr::new(""))), TOOLS);
    }

    #[test]
    fn the_message_names_the_missing_tools() {
        assert_eq!(unavailable_message(&[]), None);
        let message = unavailable_message(&["yt-dlp", "ffprobe"]).expect("missing");
        assert!(message.contains("install"), "{message}");
        assert!(
            message.ends_with("(not found: yt-dlp, ffprobe)."),
            "{message}"
        );
    }
}
