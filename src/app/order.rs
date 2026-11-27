//! The order of a tab's tiles: by file name (then clip id) until the user drags one, then as saved
//! in [`TabConfig::order`](crate::config::TabConfig::order). Clips missing from a saved order
//! follow in name order; entries of missing files are kept, so a folder that can't be read for a
//! moment doesn't lose the order.

use std::collections::HashMap;

use crate::config::ClipKey;

use super::AppState;
use super::clips::ClipId;

/// `clips` (in name order, each with its key) in the saved `order`; clips it doesn't list follow
/// in name order.
pub fn arrange<T>(clips: Vec<(ClipKey, T)>, order: &[ClipKey]) -> Vec<T> {
    let mut position = HashMap::new();
    for (index, key) in order.iter().enumerate() {
        position.entry(key).or_insert(index);
    }
    let mut clips = clips;
    clips.sort_by_key(|(key, _)| position.get(key).copied().unwrap_or(usize::MAX));
    clips.into_iter().map(|(_, clip)| clip).collect()
}

/// Moves `dragged` right before (or `after`) `target` in `order`. `shown` is the tab's clips as
/// shown (arranged by `order`); those the order doesn't list yet are added at its end first, so
/// nothing else moves. Returns whether the shown order changed; if not, `order` stays as it was.
pub fn move_entry(
    order: &mut Vec<ClipKey>,
    shown: &[ClipKey],
    dragged: &ClipKey,
    target: &ClipKey,
    after: bool,
) -> bool {
    if dragged == target || !shown.contains(dragged) || !shown.contains(target) {
        return false;
    }
    let mut moved = order.clone();
    for key in shown {
        if !moved.contains(key) {
            moved.push(key.clone());
        }
    }
    moved.retain(|key| key != dragged);
    // Both are in the order now; `target` stays after removing `dragged`.
    let index = moved.iter().position(|key| key == target).unwrap_or(0);
    moved.insert(index + usize::from(after), dragged.clone());
    let clips = shown.iter().map(|key| (key.clone(), key.clone()));
    if arrange(clips.collect(), &moved) == shown {
        return false;
    }
    *order = moved;
    true
}

/// Puts `new` right after `source` in a saved order. Without a saved order (or if it doesn't list
/// `source`) the name order already puts it near its source.
pub fn insert_after(order: &mut Vec<ClipKey>, source: &ClipKey, new: ClipKey) {
    if let Some(index) = order.iter().position(|key| key == source) {
        order.insert(index + 1, new);
    }
}

impl AppState {
    /// The tab of the clip and its key in that tab's config.
    pub fn clip_key(&self, clip: &ClipId) -> Option<(usize, ClipKey)> {
        let (tab, sound) = self.library.find(&clip.path)?;
        Some((tab, ClipKey::new(&sound.key, clip.clip)))
    }

    /// The clips of the selected tab, as shown (in its saved order).
    pub fn tab_clips(&self) -> Vec<ClipId> {
        let Some(folder) = self.library.folders().get(self.tab) else {
            return Vec::new();
        };
        let clips = (folder.sounds.iter()).flat_map(|sound| {
            let clips = self.clips_of(&sound.path).into_iter();
            clips.map(|clip| (ClipKey::new(&sound.key, clip.clip), clip))
        });
        let order = self
            .config
            .tabs
            .get(self.tab)
            .map(|tab| tab.order.as_slice());
        arrange(clips.collect(), order.unwrap_or_default())
    }

    /// Moves the dragged tile right before or `after` the `target` tile of the same tab; returns
    /// whether the order changed.
    pub fn move_clip(&mut self, dragged: &ClipId, target: &ClipId, after: bool) -> bool {
        let (Some((tab, dragged)), Some((target_tab, target))) =
            (self.clip_key(dragged), self.clip_key(target))
        else {
            return false;
        };
        if tab != target_tab || tab != self.tab {
            return false;
        }
        let shown: Vec<ClipKey> = (self.tab_clips().iter())
            .filter_map(|clip| Some(self.clip_key(clip)?.1))
            .collect();
        let Some(config) = self.config.tabs.get_mut(tab) else {
            return false;
        };
        move_entry(&mut config.order, &shown, &dragged, &target, after)
    }

    /// Forgets the selected tab's order, so its tiles are sorted by name again. Returns whether
    /// there was one.
    pub fn sort_by_name(&mut self) -> bool {
        let Some(config) = self.config.tabs.get_mut(self.tab) else {
            return false;
        };
        !std::mem::take(&mut config.order).is_empty()
    }

    /// Puts the clip `new` right after `source` in their tab's saved order.
    pub(super) fn order_after(&mut self, source: &ClipId, new: &ClipId) {
        let (Some((tab, source)), Some((_, new))) = (self.clip_key(source), self.clip_key(new))
        else {
            return;
        };
        if let Some(config) = self.config.tabs.get_mut(tab) {
            insert_after(&mut config.order, &source, new);
        }
    }

    /// Removes a deleted clip from its tab's saved order.
    pub(super) fn forget_order(&mut self, clip: &ClipId) {
        let Some((tab, key)) = self.clip_key(clip) else {
            return;
        };
        if let Some(config) = self.config.tabs.get_mut(tab) {
            config.order.retain(|entry| *entry != key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(text: &str) -> Vec<ClipKey> {
        text.split_whitespace().map(ClipKey::from).collect()
    }

    fn arranged(shown: &str, order: &str) -> String {
        let clips = keys(shown).into_iter().map(|key| (key.clone(), key));
        let arranged = arrange(clips.collect(), &keys(order));
        let names: Vec<String> = arranged.iter().map(ToString::to_string).collect();
        names.join(" ")
    }

    #[test]
    fn without_an_order_the_name_order_stays() {
        assert_eq!(arranged("a b a#1 c", ""), "a b a#1 c");
    }

    #[test]
    fn listed_clips_come_first_and_the_rest_follow_in_name_order() {
        assert_eq!(arranged("a a#1 b c d", "c a#1"), "c a#1 a b d");
        assert_eq!(arranged("a b", "gone b gone#2 a"), "b a", "stale entries");
        assert_eq!(
            arranged("a b c", "c a c"),
            "c a b",
            "the first entry counts"
        );
    }

    fn moved(order: &str, shown: &str, dragged: &str, target: &str, after: bool) -> String {
        let mut order = keys(order);
        let changed = move_entry(
            &mut order,
            &keys(shown),
            &ClipKey::from(dragged),
            &ClipKey::from(target),
            after,
        );
        let names: Vec<String> = order.iter().map(ToString::to_string).collect();
        format!("{} ({changed})", names.join(" "))
    }

    #[test]
    fn moving_creates_the_order_from_what_is_shown() {
        assert_eq!(moved("", "a b c d", "d", "b", false), "a d b c (true)");
        assert_eq!(moved("", "a b c d", "a", "c", true), "b c a d (true)");
        assert_eq!(
            moved("", "a b c", "b", "c", false),
            " (false)",
            "already there"
        );
    }

    #[test]
    fn moving_keeps_stale_entries_and_appends_unlisted_clips() {
        assert_eq!(
            moved("c gone a", "c a b new", "new", "c", false),
            "new c gone a b (true)"
        );
        assert_eq!(
            moved("b gone a", "b a", "a", "b", true),
            "b gone a (false)",
            "already right after it"
        );
    }

    #[test]
    fn moving_onto_itself_or_unknown_clips_does_nothing() {
        assert_eq!(moved("", "a b", "a", "a", true), " (false)");
        assert_eq!(moved("b a", "a b", "x", "a", true), "b a (false)");
        assert_eq!(moved("b a", "a b", "a", "gone", true), "b a (false)");
    }

    #[test]
    fn duplicates_go_right_after_their_source_in_a_saved_order() {
        let mut order = keys("b a c");
        insert_after(&mut order, &ClipKey::from("a"), ClipKey::from("a#1"));
        assert_eq!(order, keys("b a a#1 c"));
        let mut empty = Vec::new();
        insert_after(&mut empty, &ClipKey::from("a"), ClipKey::from("a#1"));
        assert!(empty.is_empty());
    }
}
