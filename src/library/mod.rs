//! The sound library: every tab is a folder, and its sounds are the audio files directly inside it
//! (not in subfolders; a tab is one folder). [`watch`] reports changes to those folders.

pub mod watch;

use std::fs;
use std::path::{Path, PathBuf};

/// File extensions (lowercase) of the formats the decoder plays.
pub const EXTENSIONS: &[&str] = &["wav", "mp3", "flac", "ogg", "m4a", "aac", "mp4"];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
}

/// An audio file in a tab's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sound {
    pub path: PathBuf,
    /// The path relative to the tab's folder, which keys the sound's metadata in the config.
    pub key: String,
    /// The file name without extension.
    pub name: String,
}

impl Sound {
    fn new(dir: &Path, path: PathBuf) -> Self {
        let key = path
            .strip_prefix(dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .into_owned();
        let name = path
            .file_stem()
            .map_or_else(|| key.clone(), |stem| stem.to_string_lossy().into_owned());
        Self { path, key, name }
    }

    /// The file extension in upper case, e.g. `MP3`.
    pub fn format(&self) -> String {
        self.path
            .extension()
            .map(|extension| extension.to_string_lossy().to_uppercase())
            .unwrap_or_default()
    }
}

/// The sounds of one tab's folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    pub dir: PathBuf,
    /// Sorted by name, ignoring case.
    pub sounds: Vec<Sound>,
    /// Why the folder couldn't be read, e.g. because it doesn't exist (anymore).
    pub error: Option<String>,
}

impl Folder {
    pub fn scan(dir: PathBuf) -> Self {
        match list_sounds(&dir) {
            Ok(sounds) => Self {
                dir,
                sounds,
                error: None,
            },
            Err(error) => Self {
                error: Some(format!("Could not read {}: {error}", dir.display())),
                dir,
                sounds: Vec::new(),
            },
        }
    }
}

fn list_sounds(dir: &Path) -> std::io::Result<Vec<Sound>> {
    let mut sounds = Vec::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() && is_supported(&path) && !is_hidden(&path) {
            sounds.push(Sound::new(dir, path));
        }
    }
    sounds.sort_by_cached_key(|sound| (sound.name.to_lowercase(), sound.key.clone()));
    Ok(sounds)
}

fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// The folders of all tabs, in tab order.
#[derive(Debug, Default)]
pub struct Library {
    folders: Vec<Folder>,
}

impl Library {
    pub fn new(dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            folders: dirs.into_iter().map(Folder::scan).collect(),
        }
    }

    pub fn folders(&self) -> &[Folder] {
        &self.folders
    }

    pub fn add(&mut self, dir: PathBuf) {
        self.folders.push(Folder::scan(dir));
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.folders.len() {
            self.folders.remove(index);
        }
    }

    /// Rescans the folders affected by a file being added, removed or renamed at `path`: the
    /// folder containing it, or the folder itself.
    pub fn changed(&mut self, path: &Path) {
        for folder in &mut self.folders {
            if folder.dir == path || path.parent() == Some(folder.dir.as_path()) {
                *folder = Folder::scan(folder.dir.clone());
            }
        }
    }

    /// The sound at `path` and the index of its tab.
    pub fn find(&self, path: &Path) -> Option<(usize, &Sound)> {
        self.folders.iter().enumerate().find_map(|(tab, folder)| {
            let sound = folder.sounds.iter().find(|sound| sound.path == path)?;
            Some((tab, sound))
        })
    }

    /// Sounds of all tabs whose name contains `query`, ignoring case; in tab order.
    pub fn search(&self, query: &str) -> Vec<&Sound> {
        let query = query.trim().to_lowercase();
        self.folders
            .iter()
            .flat_map(|folder| &folder.sounds)
            .filter(|sound| sound.name.to_lowercase().contains(&query))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, b"").expect("write");
        path
    }

    fn names(sounds: &[Sound]) -> Vec<&str> {
        sounds.iter().map(|sound| sound.name.as_str()).collect()
    }

    #[test]
    fn scanning_lists_supported_files_sorted_by_name() {
        let dir = tempfile::tempdir().expect("temp dir");
        for name in [
            "b.mp3",
            "A.WAV",
            "c.flac",
            "notes.txt",
            ".hidden.ogg",
            "song.opus",
        ] {
            touch(dir.path(), name);
        }
        fs::create_dir(dir.path().join("sub.wav")).expect("mkdir");
        touch(&dir.path().join("sub.wav"), "nested.wav");

        let folder = Folder::scan(dir.path().to_owned());

        assert_eq!(folder.error, None);
        assert_eq!(names(&folder.sounds), ["A", "b", "c"]);
        assert_eq!(folder.sounds[0].key, "A.WAV");
        assert_eq!(folder.sounds[0].path, dir.path().join("A.WAV"));
        assert_eq!(folder.sounds[0].format(), "WAV");
    }

    #[test]
    fn a_missing_folder_has_no_sounds_and_an_error() {
        let folder = Folder::scan("/does/not/exist".into());
        assert!(folder.sounds.is_empty());
        assert!(folder.error.is_some());
    }

    #[test]
    fn every_decoder_format_is_supported() {
        for extension in ["wav", "mp3", "flac", "ogg", "m4a", "aac", "mp4", "MP3"] {
            assert!(
                is_supported(Path::new(&format!("x.{extension}"))),
                "{extension}"
            );
        }
        assert!(!is_supported(Path::new("x.opus")));
        assert!(!is_supported(Path::new("wav")));
    }

    #[test]
    fn changes_rescan_the_affected_folder() {
        let a = tempfile::tempdir().expect("temp dir");
        let b = tempfile::tempdir().expect("temp dir");
        let old = touch(a.path(), "old.wav");
        touch(b.path(), "other.wav");
        let mut library = Library::new([a.path().to_owned(), b.path().to_owned()]);

        let new = a.path().join("new.wav");
        fs::rename(&old, &new).expect("rename");
        library.changed(&old);
        library.changed(&new);
        assert_eq!(names(&library.folders()[0].sounds), ["new"]);

        fs::remove_file(&new).expect("remove");
        library.changed(&new);
        assert!(library.folders()[0].sounds.is_empty());
        assert_eq!(names(&library.folders()[1].sounds), ["other"]);
    }

    #[test]
    fn a_removed_folder_is_noticed() {
        let parent = tempfile::tempdir().expect("temp dir");
        let dir = parent.path().join("tab");
        fs::create_dir(&dir).expect("mkdir");
        touch(&dir, "a.wav");
        let mut library = Library::new([dir.clone()]);

        fs::remove_dir_all(&dir).expect("remove");
        library.changed(&dir);

        assert!(library.folders()[0].sounds.is_empty());
        assert!(library.folders()[0].error.is_some());
    }

    #[test]
    fn search_matches_names_in_all_folders_ignoring_case() {
        let a = tempfile::tempdir().expect("temp dir");
        let b = tempfile::tempdir().expect("temp dir");
        touch(a.path(), "Airhorn.mp3");
        touch(a.path(), "Bruh.wav");
        touch(b.path(), "Fairy horn.ogg");
        let library = Library::new([a.path().to_owned(), b.path().to_owned()]);

        assert_eq!(
            library
                .search(" HORN ")
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["Airhorn", "Fairy horn"]
        );
        assert!(library.search("nothing").is_empty());
    }

    #[test]
    fn sounds_are_found_with_their_tab() {
        let a = tempfile::tempdir().expect("temp dir");
        let b = tempfile::tempdir().expect("temp dir");
        let path = touch(b.path(), "x.wav");
        let mut library = Library::new([a.path().to_owned(), b.path().to_owned()]);

        assert_eq!(library.find(&path).map(|(tab, _)| tab), Some(1));
        library.remove(0);
        assert_eq!(library.find(&path).map(|(tab, _)| tab), Some(0));
        assert!(library.find(Path::new("/nope.wav")).is_none());
    }
}
