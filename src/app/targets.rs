//! The apps sounds can be injected into, which one we're linked to, and when to relink to the
//! remembered one.

use crate::audio::RecordingApp;
use crate::config::SavedTarget;

/// Shown when no app records audio.
pub const NO_APPS_HINT: &str =
    "Apps only appear while they record audio, e.g. while in a Discord call.";

#[derive(Debug, Default)]
pub struct Targets {
    apps: Vec<RecordingApp>,
    /// The node we last linked to; it may have disappeared since.
    linked: Option<u32>,
}

impl Targets {
    pub fn apps(&self) -> &[RecordingApp] {
        &self.apps
    }

    pub fn set_apps(&mut self, apps: Vec<RecordingApp>) {
        self.apps = apps;
    }

    pub fn set_linked(&mut self, node_id: Option<u32>) {
        self.linked = node_id;
    }

    /// The app we're linked to, if it's still there.
    pub fn linked_app(&self) -> Option<&RecordingApp> {
        let linked = self.linked?;
        self.apps.iter().find(|app| app.node_id == linked)
    }

    /// The node to link to so that sounds reach the remembered target: `None` if we're linked to
    /// an app that is still there, or the target isn't running.
    pub fn relink(&self, saved: Option<&SavedTarget>) -> Option<u32> {
        if self.linked_app().is_some() {
            return None;
        }
        let saved = saved?;
        self.apps
            .iter()
            .find(|app| saved.matches(app))
            .map(|app| app.node_id)
    }

    /// What the target picker shows as selected.
    pub fn selected_label(&self, saved: Option<&SavedTarget>) -> String {
        match (self.linked_app(), saved) {
            (Some(app), _) => label(app),
            (None, Some(saved)) => format!("{} (not running)", saved_label(saved)),
            (None, None) => "None".to_owned(),
        }
    }
}

/// `app_name (binary)`, or just the name if the binary is unknown.
pub fn label(app: &RecordingApp) -> String {
    name_and_binary(&app.app_name, app.process_binary.as_deref())
}

fn saved_label(saved: &SavedTarget) -> String {
    name_and_binary(&saved.app_name, saved.process_binary.as_deref())
}

fn name_and_binary(name: &str, binary: Option<&str>) -> String {
    match binary {
        Some(binary) => format!("{name} ({binary})"),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(node_id: u32, name: &str, binary: &str) -> RecordingApp {
        RecordingApp {
            node_id,
            app_name: name.to_owned(),
            process_binary: Some(binary.to_owned()),
            media_name: None,
            ports: Vec::new(),
        }
    }

    fn discord() -> SavedTarget {
        SavedTarget {
            process_binary: Some("Discord".to_owned()),
            app_name: "WEBRTC VoiceEngine".to_owned(),
        }
    }

    #[test]
    fn relinks_to_the_saved_target_when_it_appears() {
        let mut targets = Targets::default();
        assert_eq!(targets.relink(Some(&discord())), None);

        targets.set_apps(vec![
            app(40, "Firefox", "firefox"),
            app(41, "WEBRTC VoiceEngine", "Discord"),
        ]);

        assert_eq!(targets.relink(Some(&discord())), Some(41));
    }

    #[test]
    fn stays_linked_while_the_linked_app_is_there() {
        let mut targets = Targets::default();
        targets.set_apps(vec![app(41, "WEBRTC VoiceEngine", "Discord")]);
        targets.set_linked(Some(41));
        assert_eq!(targets.relink(Some(&discord())), None);
    }

    #[test]
    fn relinks_when_the_linked_app_comes_back_with_a_new_node() {
        let mut targets = Targets::default();
        targets.set_apps(vec![app(41, "WEBRTC VoiceEngine", "Discord")]);
        targets.set_linked(Some(41));

        targets.set_apps(Vec::new());
        assert_eq!(targets.relink(Some(&discord())), None);
        targets.set_apps(vec![app(57, "WEBRTC VoiceEngine", "Discord")]);

        assert_eq!(targets.relink(Some(&discord())), Some(57));
    }

    #[test]
    fn never_relinks_without_a_saved_target() {
        let mut targets = Targets::default();
        targets.set_apps(vec![app(41, "WEBRTC VoiceEngine", "Discord")]);
        assert_eq!(targets.relink(None), None);
    }

    #[test]
    fn apps_with_the_same_name_but_another_binary_dont_match() {
        let mut targets = Targets::default();
        targets.set_apps(vec![app(41, "WEBRTC VoiceEngine", "chromium")]);
        assert_eq!(targets.relink(Some(&discord())), None);
    }

    #[test]
    fn labels_show_name_and_binary_and_whether_the_target_runs() {
        let mut targets = Targets::default();
        assert_eq!(targets.selected_label(None), "None");
        assert_eq!(
            targets.selected_label(Some(&discord())),
            "WEBRTC VoiceEngine (Discord) (not running)"
        );
        targets.set_apps(vec![app(41, "WEBRTC VoiceEngine", "Discord")]);
        targets.set_linked(Some(41));
        assert_eq!(
            targets.selected_label(Some(&discord())),
            "WEBRTC VoiceEngine (Discord)"
        );
        let unknown_binary = RecordingApp {
            process_binary: None,
            ..app(1, "pw-record", "")
        };
        assert_eq!(label(&unknown_binary), "pw-record");
    }
}
