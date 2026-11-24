//! Downloads with `yt-dlp` as a child process (no shell): it extracts the audio as MP3 with
//! `ffmpeg` and reports progress in lines we tell it to print (see [`command`]).
//!
//! Every download works in a hidden staging folder inside the tab folder (yt-dlp's `temp` path),
//! so partial and intermediate files never show up as sounds; yt-dlp moves the finished MP3 out of
//! it with a rename. Cancelling kills yt-dlp's process group (with its `ffmpeg`) and removes the
//! staging folder.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;

use tempfile::TempDir;

use super::{DownloadEvent, Downloader, Stage, TOOLS, missing_tools, unavailable_message};

const PROGRAM: &str = "yt-dlp";
/// Prefixes of the lines we make yt-dlp print on stdout.
const DOWNLOAD_LINE: &str = "soundboars-download ";
const CONVERTING_LINE: &str = "soundboars-converting";
const DONE_LINE: &str = "soundboars-done ";
/// The staging folders' name prefix; hidden, so the library ignores them.
const STAGING_PREFIX: &str = ".soundboars-download-";
/// Longest file name (without extension) yt-dlp may write.
const MAX_NAME_LENGTH: usize = 120;
/// How many of yt-dlp's last stderr lines to keep for the error message.
const STDERR_LINES: usize = 20;

/// The `yt-dlp` command line that downloads the audio of `url` as MP3 into `folder`, working in
/// `staging`. The file is named after the page's title (sanitized and shortened); an existing file
/// is never overwritten (yt-dlp reports it as done instead).
pub fn command(url: &str, folder: &Path, staging: &Path) -> Command {
    let mut command = Command::new(PROGRAM);
    command
        // Ignore the user's yt-dlp config: it could change the output name or format.
        .args(["--ignore-config", "--no-playlist", "--no-colors"])
        .args(["--extract-audio", "--audio-format", "mp3"])
        .args(["--windows-filenames", "--trim-filenames"])
        .arg(MAX_NAME_LENGTH.to_string())
        .arg("--no-overwrites")
        // `--print` implies `--quiet`; `--progress` keeps the progress lines.
        .args(["--newline", "--progress", "--progress-template"])
        .arg(format!(
            "download:{DOWNLOAD_LINE}%(progress.status)s %(progress.downloaded_bytes)s \
             %(progress.total_bytes)s %(progress.total_bytes_estimate)s"
        ))
        .args(["--print", &format!("post_process:{CONVERTING_LINE}")])
        .args(["--print", &format!("after_move:{DONE_LINE}%(filepath)s")])
        .arg("--paths")
        .arg(prefixed("home:", folder))
        .arg("--paths")
        .arg(prefixed("temp:", staging))
        .args(["--output", "%(title)s.%(ext)s", "--", url]);
    command
}

fn prefixed(prefix: &str, path: &Path) -> std::ffi::OsString {
    let mut arg = std::ffi::OsString::from(prefix);
    arg.push(path);
    arg
}

/// A line of yt-dlp's stdout that tells us something.
#[derive(Debug, Clone, PartialEq)]
enum Report {
    Downloading {
        done: u64,
        total: Option<u64>,
    },
    Converting,
    /// The finished file.
    Done(PathBuf),
}

fn parse(line: &str) -> Option<Report> {
    if let Some(path) = line.strip_prefix(DONE_LINE) {
        return Some(Report::Done(PathBuf::from(path)));
    }
    if line == CONVERTING_LINE {
        return Some(Report::Converting);
    }
    let mut fields = line.strip_prefix(DOWNLOAD_LINE)?.split(' ');
    let _status = fields.next()?;
    // Unknown fields are `NA`; bytes may be floats.
    let mut number = || {
        let value: f64 = fields.next()?.parse().ok()?;
        Some(value as u64)
    };
    let done = number()?;
    let total = number().or_else(number).filter(|&total| total > 0);
    Some(Report::Downloading { done, total })
}

/// Follows yt-dlp's output and turns it into [`DownloadEvent`]s.
#[derive(Debug)]
struct Tracker {
    stage: Stage,
    file: Option<PathBuf>,
    stderr: VecDeque<String>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            stage: Stage::Starting,
            file: None,
            stderr: VecDeque::new(),
        }
    }
}

impl Tracker {
    /// The new stage, if `line` changes it.
    fn stdout(&mut self, line: &str) -> Option<DownloadEvent> {
        let stage = match parse(line)? {
            Report::Downloading { done, total } => {
                Stage::Downloading(total.map(|total| (done as f32 / total as f32).min(1.0)))
            }
            Report::Converting => Stage::Converting,
            Report::Done(path) => {
                self.file = Some(path);
                return None;
            }
        };
        (stage != self.stage).then(|| {
            self.stage = stage;
            DownloadEvent::Progress(stage)
        })
    }

    fn stderr(&mut self, line: &str) {
        if self.stderr.len() == STDERR_LINES {
            self.stderr.pop_front();
        }
        self.stderr.push_back(line.to_owned());
    }

    /// The outcome once yt-dlp exited with `status`. The file is `folder` joined with the name
    /// yt-dlp printed, so its path matches the library's.
    fn finish(self, status: ExitStatus, folder: &Path) -> DownloadEvent {
        let file = self.file.as_deref().and_then(Path::file_name);
        match file {
            Some(name) if status.success() => DownloadEvent::Done(folder.join(name)),
            None if status.success() => {
                DownloadEvent::Failed("yt-dlp finished without saving a file".to_owned())
            }
            _ => DownloadEvent::Failed(error_message(&self.stderr, status)),
        }
    }
}

/// What went wrong, from yt-dlp's stderr: its `ERROR:` lines without the technical cause, else
/// its last lines, else the exit status.
fn error_message(stderr: &VecDeque<String>, status: ExitStatus) -> String {
    let errors: Vec<&str> = stderr
        .iter()
        .filter_map(|line| line.strip_prefix("ERROR: "))
        .map(|error| error.split(" (caused by ").next().unwrap_or(error))
        .collect();
    if !errors.is_empty() {
        return errors.join("\n");
    }
    let last: Vec<&str> = stderr
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    match last.len() {
        0 => format!("yt-dlp failed ({status})"),
        n => last[n.saturating_sub(3)..].join("\n"),
    }
}

/// [`Downloader`] with the `yt-dlp` on `$PATH`.
pub struct YtDlp {
    unavailable: Option<String>,
    running: Option<Running>,
    /// Events of downloads that failed to start.
    queued: Vec<DownloadEvent>,
}

impl YtDlp {
    /// Checks once whether yt-dlp and ffmpeg are installed.
    pub fn new() -> Self {
        let path = std::env::var_os("PATH");
        let unavailable = unavailable_message(&missing_tools(TOOLS, path.as_deref()));
        if let Some(reason) = &unavailable {
            tracing::info!("{reason}");
        }
        Self {
            unavailable,
            running: None,
            queued: Vec::new(),
        }
    }
}

impl Default for YtDlp {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader for YtDlp {
    fn unavailable(&self) -> Option<String> {
        self.unavailable.clone()
    }

    fn start(&mut self, url: &str, folder: &Path) {
        self.cancel();
        match Running::start(url, folder) {
            Ok(running) => self.running = Some(running),
            Err(error) => self.queued.push(DownloadEvent::Failed(format!(
                "Could not start yt-dlp: {error}"
            ))),
        }
    }

    fn cancel(&mut self) {
        // Dropping it kills yt-dlp and removes the staging folder.
        self.running = None;
    }

    fn poll(&mut self) -> Vec<DownloadEvent> {
        let mut events = std::mem::take(&mut self.queued);
        if let Some(running) = &mut self.running {
            events.extend(running.read_output());
            if let Some(outcome) = running.outcome() {
                events.push(outcome);
                self.running = None;
            }
        }
        events
    }
}

/// A line of yt-dlp's output.
enum Output {
    Stdout(String),
    Stderr(String),
}

/// A running yt-dlp. Dropping it kills it and removes its staging folder.
struct Running {
    child: Child,
    output: mpsc::Receiver<Output>,
    /// Both pipes reached their end.
    output_closed: bool,
    tracker: Tracker,
    folder: PathBuf,
    /// Removed on drop.
    _staging: TempDir,
}

impl Running {
    fn start(url: &str, folder: &Path) -> std::io::Result<Self> {
        let staging = tempfile::Builder::new()
            .prefix(STAGING_PREFIX)
            .tempdir_in(folder)?;
        let mut child = command(url, folder, staging.path())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Its own process group, so cancelling can kill its ffmpeg, too.
            .process_group(0)
            .spawn()?;
        let (sender, output) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            forward_lines(stdout, sender.clone(), Output::Stdout)?;
        }
        if let Some(stderr) = child.stderr.take() {
            forward_lines(stderr, sender, Output::Stderr)?;
        }
        Ok(Self {
            child,
            output,
            output_closed: false,
            tracker: Tracker::default(),
            folder: folder.to_owned(),
            _staging: staging,
        })
    }

    fn read_output(&mut self) -> Vec<DownloadEvent> {
        let mut events = Vec::new();
        loop {
            match self.output.try_recv() {
                Ok(Output::Stdout(line)) => events.extend(self.tracker.stdout(&line)),
                Ok(Output::Stderr(line)) => self.tracker.stderr(&line),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.output_closed = true;
                    break;
                }
            }
        }
        events
    }

    /// Done or failed, once yt-dlp exited and all its output was read.
    fn outcome(&mut self) -> Option<DownloadEvent> {
        if !self.output_closed {
            return None;
        }
        let status = match self.child.try_wait() {
            Ok(status) => status?,
            Err(error) => {
                return Some(DownloadEvent::Failed(format!(
                    "Lost track of yt-dlp: {error}"
                )));
            }
        };
        let tracker = std::mem::take(&mut self.tracker);
        Some(tracker.finish(status, &self.folder))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        // Only while it runs: once reaped, its process group ID may belong to someone else.
        if let Ok(None) = self.child.try_wait() {
            kill_process_group(&self.child);
            // Returns right away: it was just killed.
            let _ = self.child.wait();
        }
    }
}

fn kill_process_group(child: &Child) {
    let Ok(group) = libc::pid_t::try_from(child.id()) else {
        return;
    };
    // SAFETY: `kill` takes no pointers; a negative PID addresses the child's process group, which
    // `process_group(0)` made its own and which can't be reused while the child isn't reaped.
    let result = unsafe { libc::kill(-group, libc::SIGKILL) };
    if result != 0 {
        tracing::warn!("could not stop yt-dlp: {}", std::io::Error::last_os_error());
    }
}

/// Sends each line of `pipe` (lossily decoded) on a thread of its own until the pipe closes.
fn forward_lines(
    pipe: impl Read + Send + 'static,
    sender: mpsc::Sender<Output>,
    wrap: fn(String) -> Output,
) -> std::io::Result<()> {
    thread::Builder::new()
        .name("yt-dlp-output".to_owned())
        .spawn(move || {
            for line in BufReader::new(pipe).split(b'\n') {
                let Ok(line) = line else { return };
                let line = String::from_utf8_lossy(&line).trim_end().to_owned();
                if sender.send(wrap(line)).is_err() {
                    return;
                }
            }
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    /// yt-dlp 2025.04.30's stdout for a 576078 bytes WAV from a local HTTP server.
    const SUCCESS: &str = "\
soundboars-download downloading 1024 576078 NA
soundboars-download downloading 3072 576078 NA
soundboars-download downloading 261120 576078 NA
soundboars-download downloading 576078 576078 NA
soundboars-download finished 576078 576078 NA
soundboars-converting
soundboars-done /s/out/tone.mp3";

    /// Its stderr for a missing file.
    const NOT_FOUND: &str = "ERROR: [generic] Unable to download webpage: HTTP Error 404: File \
                             not found (caused by <HTTPError 404: File not found>)";

    fn exit(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    fn stderr(lines: &[&str]) -> VecDeque<String> {
        lines.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn the_command_line_is_exactly_as_intended() {
        let command = command(
            "https://example.com/watch?v=1",
            Path::new("/sounds/My: Memes"),
            Path::new("/sounds/My: Memes/.soundboars-download-x"),
        );
        let args: Vec<&str> = command
            .get_args()
            .map(|arg| arg.to_str().expect("UTF-8"))
            .collect();

        assert_eq!(command.get_program(), "yt-dlp");
        assert_eq!(
            args,
            [
                "--ignore-config",
                "--no-playlist",
                "--no-colors",
                "--extract-audio",
                "--audio-format",
                "mp3",
                "--windows-filenames",
                "--trim-filenames",
                "120",
                "--no-overwrites",
                "--newline",
                "--progress",
                "--progress-template",
                "download:soundboars-download %(progress.status)s %(progress.downloaded_bytes)s \
                 %(progress.total_bytes)s %(progress.total_bytes_estimate)s",
                "--print",
                "post_process:soundboars-converting",
                "--print",
                "after_move:soundboars-done %(filepath)s",
                "--paths",
                "home:/sounds/My: Memes",
                "--paths",
                "temp:/sounds/My: Memes/.soundboars-download-x",
                "--output",
                "%(title)s.%(ext)s",
                "--",
                "https://example.com/watch?v=1",
            ]
        );
    }

    #[test]
    fn progress_lines_are_parsed() {
        assert_eq!(
            parse("soundboars-download downloading 1024 576078 NA"),
            Some(Report::Downloading {
                done: 1024,
                total: Some(576_078)
            })
        );
        assert_eq!(
            parse("soundboars-download downloading 512 NA 2048.5"),
            Some(Report::Downloading {
                done: 512,
                total: Some(2048)
            }),
            "falls back to the estimate"
        );
        assert_eq!(
            parse("soundboars-download downloading 512 NA NA"),
            Some(Report::Downloading {
                done: 512,
                total: None
            })
        );
        assert_eq!(parse("soundboars-converting"), Some(Report::Converting));
        assert_eq!(
            parse("soundboars-done /a/b c.mp3"),
            Some(Report::Done("/a/b c.mp3".into()))
        );
        for other in [
            "",
            "[download] 10%",
            "soundboars-download downloading NA NA NA",
        ] {
            assert_eq!(parse(other), None, "{other:?}");
        }
    }

    #[test]
    fn stages_follow_the_output_and_end_with_the_file() {
        let mut tracker = Tracker::default();
        let events: Vec<DownloadEvent> = SUCCESS
            .lines()
            .filter_map(|line| tracker.stdout(line))
            .collect();

        let downloading =
            |fraction: f32| DownloadEvent::Progress(Stage::Downloading(Some(fraction)));
        assert_eq!(
            events,
            [
                downloading(1024.0 / 576_078.0),
                downloading(3072.0 / 576_078.0),
                downloading(261_120.0 / 576_078.0),
                downloading(1.0),
                DownloadEvent::Progress(Stage::Converting),
            ],
            "unchanged stages are not repeated"
        );
        assert_eq!(
            tracker.finish(exit(0), Path::new("/sounds")),
            DownloadEvent::Done("/sounds/tone.mp3".into())
        );
    }

    #[test]
    fn failures_explain_themselves() {
        let mut tracker = Tracker::default();
        tracker.stderr("WARNING: [generic] Falling back on generic information extractor");
        tracker.stderr(NOT_FOUND);
        assert_eq!(
            tracker.finish(exit(1), Path::new("/sounds")),
            DownloadEvent::Failed(
                "[generic] Unable to download webpage: HTTP Error 404: File not found".to_owned()
            )
        );

        let no_file = Tracker::default().finish(exit(0), Path::new("/sounds"));
        assert!(matches!(no_file, DownloadEvent::Failed(_)), "{no_file:?}");

        let mut tracker = Tracker::default();
        tracker.stdout("soundboars-done /sounds/x.mp3");
        let failed = tracker.finish(exit(2), Path::new("/sounds"));
        assert!(matches!(failed, DownloadEvent::Failed(_)), "{failed:?}");
    }

    #[test]
    fn without_error_lines_the_last_lines_or_the_status_explain() {
        let lines = stderr(&["a", "Traceback:", "  File x", "", "KeyError: 'y'", " "]);
        assert_eq!(
            error_message(&lines, exit(1)),
            "Traceback:\nFile x\nKeyError: 'y'"
        );
        assert_eq!(
            error_message(&stderr(&[]), ExitStatus::from_raw(9)),
            "yt-dlp failed (signal: 9 (SIGKILL))"
        );
        let errors = stderr(&["ERROR: one", "noise", "ERROR: two"]);
        assert_eq!(error_message(&errors, exit(1)), "one\ntwo");
    }

    #[test]
    fn the_stderr_tail_is_bounded() {
        let mut tracker = Tracker::default();
        for index in 0..100 {
            tracker.stderr(&index.to_string());
        }
        assert_eq!(tracker.stderr.len(), STDERR_LINES);
        assert_eq!(tracker.stderr.back().map(String::as_str), Some("99"));
    }
}
