//! End-to-end tests of [`YtDlp`] with the real `yt-dlp` and `ffmpeg`, offline: the media comes
//! from an HTTP server on 127.0.0.1 in this process (yt-dlp's generic extractor takes the direct
//! link). Runs only with `SOUNDBOARS_REQUIRE_PIPEWIRE=1`, i.e. in the container that has both tools
//! (see `common::pipewire_required`).

mod common;

use std::f32::consts::TAU;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use soundboars::audio::decode::probe_duration;
use soundboars::import::ytdlp::YtDlp;
use soundboars::import::{Browser, BrowserCookies, DownloadEvent, Downloader, Stage};

const TIMEOUT: Duration = Duration::from_secs(60);

/// 16-bit stereo 48 kHz WAV of a 440 Hz tone.
fn wav(seconds: f32) -> Vec<u8> {
    let frames = (48_000.0 * seconds) as u32;
    let data_len = frames * 4;
    let mut bytes = Vec::with_capacity(44 + data_len as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes()); // PCM
    bytes.extend_from_slice(&2u16.to_le_bytes()); // channels
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&(48_000u32 * 4).to_le_bytes()); // bytes per second
    bytes.extend_from_slice(&4u16.to_le_bytes()); // block align
    bytes.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        let sample = (TAU * 440.0 * frame as f32 / 48_000.0).sin() * 0.5;
        let sample = (sample * f32::from(i16::MAX)) as i16;
        bytes.extend_from_slice(&sample.to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// Serves `body` as `/tone.wav` (everything else is a 404), optionally slowly: a chunk every
/// 50 ms. Returns the URL base.
fn serve(body: Vec<u8>, chunk: Option<usize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("address").port();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let body = body.clone();
            thread::spawn(move || respond(stream, &body, chunk));
        }
    });
    format!("http://127.0.0.1:{port}")
}

fn respond(mut stream: TcpStream, body: &[u8], chunk: Option<usize>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Skip the headers.
    let mut line = String::new();
    while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
        line.clear();
    }
    let path = request_line.split(' ').nth(1).unwrap_or_default();
    if path != "/tone.wav" {
        let _ = stream
            .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        return;
    }
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(header.as_bytes()).is_err() {
        return;
    }
    let chunk_size = chunk.unwrap_or(body.len());
    for piece in body.chunks(chunk_size) {
        if stream.write_all(piece).is_err() {
            return; // The client went away (e.g. the download was cancelled).
        }
        if chunk.is_some() {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// Polls until the download is done or failed; returns all events.
fn run_to_end(downloader: &mut YtDlp) -> Vec<DownloadEvent> {
    let deadline = Instant::now() + TIMEOUT;
    let mut events = Vec::new();
    loop {
        events.extend(downloader.poll());
        if matches!(
            events.last(),
            Some(DownloadEvent::Done(_) | DownloadEvent::Failed(_))
        ) {
            return events;
        }
        assert!(Instant::now() < deadline, "no outcome: {events:?}");
        thread::sleep(Duration::from_millis(20));
    }
}

fn files_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

/// How many processes have `arg` on their command line.
fn processes_with_arg(arg: &str) -> usize {
    let processes = fs::read_dir("/proc").expect("procfs").flatten();
    processes
        .filter_map(|entry| fs::read(entry.path().join("cmdline")).ok())
        .filter(|cmdline| {
            cmdline
                .split(|&byte| byte == 0)
                .any(|part| part == arg.as_bytes())
        })
        .count()
}

fn downloader() -> YtDlp {
    let downloader = YtDlp::new();
    assert_eq!(
        downloader.unavailable(),
        None,
        "the container has the tools"
    );
    downloader
}

#[test]
fn downloads_audio_as_mp3_into_the_folder() {
    if !common::pipewire_required() {
        return;
    }
    let base = serve(wav(2.0), None);
    let folder = tempfile::tempdir().expect("temp dir");
    let mut downloader = downloader();

    downloader.start(&format!("{base}/tone.wav"), folder.path(), None);
    let events = run_to_end(&mut downloader);

    let mp3: PathBuf = folder.path().join("tone.mp3");
    assert_eq!(events.last(), Some(&DownloadEvent::Done(mp3.clone())));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, DownloadEvent::Progress(Stage::Downloading(Some(_))))),
        "{events:?}"
    );
    assert!(
        events.contains(&DownloadEvent::Progress(Stage::Converting)),
        "{events:?}"
    );
    assert_eq!(
        files_in(folder.path()),
        ["tone.mp3"],
        "no staging leftovers"
    );
    let duration = probe_duration(&mp3).expect("decodes").as_secs_f32();
    assert!((duration - 2.0).abs() < 0.15, "{duration} s");
}

#[test]
fn an_existing_file_is_not_overwritten() {
    if !common::pipewire_required() {
        return;
    }
    let base = serve(wav(0.5), None);
    let folder = tempfile::tempdir().expect("temp dir");
    let existing = folder.path().join("tone.mp3");
    let mine = fs::read(common::fixture("tone.mp3")).expect("fixture");
    fs::write(&existing, &mine).expect("write");
    let mut downloader = downloader();

    downloader.start(&format!("{base}/tone.wav"), folder.path(), None);
    let events = run_to_end(&mut downloader);

    assert_eq!(events.last(), Some(&DownloadEvent::Done(existing.clone())));
    assert_eq!(fs::read(&existing).expect("read"), mine);
    assert_eq!(files_in(folder.path()), ["tone.mp3"]);
}

#[test]
fn a_missing_page_fails_with_a_readable_message() {
    if !common::pipewire_required() {
        return;
    }
    let base = serve(Vec::new(), None);
    let folder = tempfile::tempdir().expect("temp dir");
    let mut downloader = downloader();

    downloader.start(&format!("{base}/gone.wav"), folder.path(), None);
    let events = run_to_end(&mut downloader);

    let Some(DownloadEvent::Failed(message)) = events.last() else {
        panic!("{events:?}");
    };
    assert!(message.contains("HTTP Error 404"), "{message}");
    assert!(!message.contains("ERROR:"), "{message}");
    assert!(files_in(folder.path()).is_empty());
}

#[test]
fn a_browser_profile_without_cookies_fails_with_a_readable_message() {
    if !common::pipewire_required() {
        return;
    }
    let base = serve(wav(0.5), None);
    let folder = tempfile::tempdir().expect("temp dir");
    let profile = tempfile::tempdir().expect("temp dir");
    let profile = profile.path().to_str().expect("UTF-8").to_owned();
    let cookies = BrowserCookies::new(Browser::Firefox, Some(profile.clone()));
    let mut downloader = downloader();

    downloader.start(&format!("{base}/tone.wav"), folder.path(), Some(&cookies));
    let events = run_to_end(&mut downloader);

    let Some(DownloadEvent::Failed(message)) = events.last() else {
        panic!("{events:?}");
    };
    let expected = format!("could not find firefox cookies database in '{profile}'");
    assert_eq!(message, &expected);
    assert!(files_in(folder.path()).is_empty());
}

#[test]
fn cancelling_stops_the_download_and_leaves_no_files() {
    if !common::pipewire_required() {
        return;
    }
    // 30 s of audio (5.8 MB) at 320 KB/s: far from done when cancelled.
    let base = serve(wav(30.0), Some(16 * 1024));
    let folder = tempfile::tempdir().expect("temp dir");
    let mut downloader = downloader();

    let url = format!("{base}/tone.wav");
    downloader.start(&url, folder.path(), None);
    let deadline = Instant::now() + TIMEOUT;
    while !downloader
        .poll()
        .iter()
        .any(|event| matches!(event, DownloadEvent::Progress(Stage::Downloading(_))))
    {
        assert!(Instant::now() < deadline, "the download never started");
        thread::sleep(Duration::from_millis(20));
    }
    assert!(processes_with_arg(&url) > 0);
    downloader.cancel();

    assert_eq!(processes_with_arg(&url), 0, "yt-dlp was killed");
    assert!(files_in(folder.path()).is_empty());
    thread::sleep(Duration::from_millis(300));
    assert_eq!(downloader.poll(), []);
    assert!(files_in(folder.path()).is_empty());
}
