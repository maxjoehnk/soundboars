//! The "Rename sound" dialog's state: the edited name of one clip. Only one is open at a time;
//! renaming another clip replaces it.

use crate::config::SoundConfig;

use super::clips::{self, ClipId};

#[derive(Debug, Clone, PartialEq)]
pub struct Rename {
    pub clip: ClipId,
    /// The file name, shown read-only.
    pub file_name: String,
    /// As edited (see [`Self::saved_name`]).
    pub name: String,
    /// The name field should take the keyboard focus (it just opened).
    pub focus: bool,
}

/// Something the user did in the dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum RenameAction {
    Name(String),
    /// The name field got the keyboard focus.
    Focused,
    Save,
    /// Cancel: close without saving.
    Close,
}

impl Rename {
    /// Renames `clip`, starting from the name it shows now.
    pub fn open(clip: &ClipId, saved: &SoundConfig) -> Self {
        let path = &clip.path;
        let file_name = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let default = clips::default_name(&clips::file_stem(path), clip.clip);
        Self {
            clip: clip.clone(),
            file_name,
            name: saved.name.clone().unwrap_or(default),
            focus: true,
        }
    }

    /// Edits the copy; saving and closing are the app's business and ignored here.
    pub fn edit(&mut self, action: RenameAction) {
        match action {
            RenameAction::Name(name) => self.name = name,
            RenameAction::Focused => self.focus = false,
            RenameAction::Save | RenameAction::Close => {}
        }
    }

    /// What the clip shows without a name of its own (the field's hint).
    pub fn default_name(&self) -> String {
        clips::default_name(&clips::file_stem(&self.clip.path), self.clip.clip)
    }

    /// The name to save, like "Edit sound" does: `None` when it's blank or the default name.
    pub fn saved_name(&self) -> Option<String> {
        clips::saved_name(&self.name, &self.default_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_with_the_shown_name_and_focuses_the_field() {
        let clip = ClipId::new("/sounds/Air horn.mp3", 1);
        let rename = Rename::open(&clip, &SoundConfig::default());
        assert_eq!(rename.name, "Air horn (2)");
        assert_eq!(rename.default_name(), "Air horn (2)");
        assert_eq!(rename.file_name, "Air horn.mp3");
        assert!(rename.focus);

        let named = SoundConfig {
            name: Some("Intro".to_owned()),
            ..SoundConfig::default()
        };
        assert_eq!(Rename::open(&clip, &named).name, "Intro");
    }

    #[test]
    fn blank_or_default_names_save_as_none() {
        let mut rename = Rename::open(
            &ClipId::file("/sounds/Airhorn.wav"),
            &SoundConfig::default(),
        );
        assert_eq!(rename.saved_name(), None);
        rename.edit(RenameAction::Focused);
        assert!(!rename.focus);
        rename.edit(RenameAction::Name(" Horn of doom ".to_owned()));
        assert_eq!(rename.saved_name().as_deref(), Some("Horn of doom"));
        rename.edit(RenameAction::Name("Airhorn".to_owned()));
        assert_eq!(rename.saved_name(), None);
        rename.edit(RenameAction::Name("   ".to_owned()));
        assert_eq!(rename.saved_name(), None);
    }
}
