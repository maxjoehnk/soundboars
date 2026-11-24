//! Watching the tab folders for files being added, removed, renamed or rewritten.

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::event::ModifyKind;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// A change in a watched folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderEvent {
    /// A file (or the folder itself) at this path was added, removed or renamed.
    Listing(PathBuf),
    /// The contents of the file at this path may have changed.
    Contents(PathBuf),
}

/// Reports changes in the folders it watches. The app polls it once per frame.
pub trait FolderWatcher {
    fn watch(&mut self, dir: &Path) -> Result<(), notify::Error>;
    fn unwatch(&mut self, dir: &Path);
    /// The changes since the last call.
    fn poll(&mut self) -> Vec<FolderEvent>;
}

/// Watches folders (not their subfolders) with `notify` (inotify on Linux).
pub struct NotifyWatcher {
    watcher: RecommendedWatcher,
    events: mpsc::Receiver<FolderEvent>,
}

impl NotifyWatcher {
    pub fn new() -> Result<Self, notify::Error> {
        let (sender, events) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            match event {
                // Fails only if the watcher (and with it the receiver) is being dropped.
                Ok(event) => translate(event).into_iter().for_each(|event| {
                    let _ = sender.send(event);
                }),
                Err(error) => tracing::warn!("watching folders: {error}"),
            }
        })?;
        Ok(Self { watcher, events })
    }
}

impl FolderWatcher for NotifyWatcher {
    fn watch(&mut self, dir: &Path) -> Result<(), notify::Error> {
        self.watcher.watch(dir, RecursiveMode::NonRecursive)
    }

    fn unwatch(&mut self, dir: &Path) {
        // Fails if `dir` wasn't watched (e.g. it didn't exist), which is fine.
        let _ = self.watcher.unwatch(dir);
    }

    fn poll(&mut self) -> Vec<FolderEvent> {
        self.events.try_iter().collect()
    }
}

/// What a `notify` event means for the library. A file that was replaced by renaming another one
/// over it has new contents, so listing changes report the contents as changed, too.
fn translate(event: notify::Event) -> Vec<FolderEvent> {
    let listing = match event.kind {
        EventKind::Create(_)
        | EventKind::Remove(_)
        | EventKind::Modify(ModifyKind::Name(_))
        | EventKind::Any
        | EventKind::Other => true,
        EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any | ModifyKind::Other) => false,
        EventKind::Modify(ModifyKind::Metadata(_)) | EventKind::Access(_) => return Vec::new(),
    };
    event
        .paths
        .into_iter()
        .flat_map(|path| {
            let contents = FolderEvent::Contents(path.clone());
            if listing {
                vec![FolderEvent::Listing(path), contents]
            } else {
                vec![contents]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{Duration, Instant};

    use notify::event::{CreateKind, DataChange, MetadataKind, RenameMode};

    use super::*;

    fn event(kind: EventKind, path: &str) -> notify::Event {
        notify::Event::new(kind).add_path(path.into())
    }

    #[test]
    fn additions_removals_and_renames_change_the_listing() {
        for kind in [
            EventKind::Create(CreateKind::File),
            EventKind::Remove(notify::event::RemoveKind::File),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
        ] {
            assert_eq!(
                translate(event(kind, "/a/b.wav")),
                [
                    FolderEvent::Listing("/a/b.wav".into()),
                    FolderEvent::Contents("/a/b.wav".into())
                ],
                "{kind:?}"
            );
        }
    }

    #[test]
    fn writes_change_the_contents_and_metadata_changes_nothing() {
        assert_eq!(
            translate(event(
                EventKind::Modify(ModifyKind::Data(DataChange::Content)),
                "/a/b.wav"
            )),
            [FolderEvent::Contents("/a/b.wav".into())]
        );
        assert!(
            translate(event(
                EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
                "/a/b.wav"
            ))
            .is_empty()
        );
    }

    #[test]
    fn real_file_changes_are_reported() {
        let dir = tempfile::tempdir().expect("temp dir");
        let mut watcher = NotifyWatcher::new().expect("watcher");
        watcher.watch(dir.path()).expect("watch");
        let path = dir.path().join("new.wav");
        fs::write(&path, b"x").expect("write");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while !events.contains(&FolderEvent::Listing(path.clone())) {
            assert!(Instant::now() < deadline, "no event; got {events:?}");
            std::thread::sleep(Duration::from_millis(10));
            events.extend(watcher.poll());
        }
    }
}
