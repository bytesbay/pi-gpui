//! Side panels: two docks, left and right, around the main area. Each dock
//! holds some panels and shows one at a time. Panels can move between docks;
//! the layout is saved with the rest of the app state.

use serde_json::{Value, json};

use crate::ui::IconName;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelKind {
    Sessions,
    Files,
    Diff,
}

impl PanelKind {
    pub const ALL: [PanelKind; 3] = [PanelKind::Sessions, PanelKind::Files, PanelKind::Diff];

    fn key(self) -> &'static str {
        match self {
            PanelKind::Sessions => "sessions",
            PanelKind::Files => "files",
            PanelKind::Diff => "diff",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub fn title(self) -> &'static str {
        match self {
            PanelKind::Sessions => "Projects",
            PanelKind::Files => "Files",
            PanelKind::Diff => "Changes",
        }
    }

    pub fn icon(self) -> IconName {
        match self {
            PanelKind::Sessions => IconName::Message,
            PanelKind::Files => IconName::Folder,
            PanelKind::Diff => IconName::Diff,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DockSide {
    Left,
    Right,
}

impl DockSide {
    pub fn other(self) -> Self {
        match self {
            DockSide::Left => DockSide::Right,
            DockSide::Right => DockSide::Left,
        }
    }

    fn key(self) -> &'static str {
        match self {
            DockSide::Left => "left",
            DockSide::Right => "right",
        }
    }

    pub fn min_width(self) -> f32 {
        match self {
            DockSide::Left => 200.,
            DockSide::Right => 260.,
        }
    }

    pub fn max_width(self) -> f32 {
        match self {
            DockSide::Left => 480.,
            DockSide::Right => 760.,
        }
    }
}

pub struct Dock {
    pub panels: Vec<PanelKind>,
    pub active: Option<PanelKind>,
    pub open: bool,
    pub width: f32,
}

impl Dock {
    /// Open and holding something to show.
    pub fn is_shown(&self) -> bool {
        self.open && self.active.is_some()
    }

    fn fix_active(&mut self) {
        if !self.active.is_some_and(|kind| self.panels.contains(&kind)) {
            self.active = self.panels.first().copied();
        }
    }
}

pub struct Docks {
    pub left: Dock,
    pub right: Dock,
}

impl Default for Docks {
    /// Projects on the left as before; files and changes on the right,
    /// closed until asked for.
    fn default() -> Self {
        Self {
            left: Dock {
                panels: vec![PanelKind::Sessions],
                active: Some(PanelKind::Sessions),
                open: true,
                width: 272.,
            },
            right: Dock {
                panels: vec![PanelKind::Files, PanelKind::Diff],
                active: Some(PanelKind::Files),
                open: false,
                width: 360.,
            },
        }
    }
}

impl Docks {
    pub fn dock(&self, side: DockSide) -> &Dock {
        match side {
            DockSide::Left => &self.left,
            DockSide::Right => &self.right,
        }
    }

    pub fn dock_mut(&mut self, side: DockSide) -> &mut Dock {
        match side {
            DockSide::Left => &mut self.left,
            DockSide::Right => &mut self.right,
        }
    }

    pub fn side_of(&self, kind: PanelKind) -> DockSide {
        if self.left.panels.contains(&kind) {
            DockSide::Left
        } else {
            DockSide::Right
        }
    }

    pub fn is_visible(&self, kind: PanelKind) -> bool {
        let dock = self.dock(self.side_of(kind));
        dock.open && dock.active == Some(kind)
    }

    /// Opens the panel's dock with the panel in front.
    pub fn show(&mut self, kind: PanelKind) {
        let dock = self.dock_mut(self.side_of(kind));
        dock.open = true;
        dock.active = Some(kind);
    }

    pub fn move_panel(&mut self, kind: PanelKind, to: DockSide) {
        let from = self.side_of(kind);
        if from == to {
            return;
        }
        self.dock_mut(from).panels.retain(|panel| *panel != kind);
        self.dock_mut(from).fix_active();
        let dock = self.dock_mut(to);
        dock.panels.push(kind);
        dock.active = Some(kind);
        dock.open = true;
    }

    pub fn to_json(&self) -> Value {
        let dock = |dock: &Dock| {
            json!({
                "panels": dock.panels.iter().map(|kind| kind.key()).collect::<Vec<_>>(),
                "active": dock.active.map(PanelKind::key),
                "open": dock.open,
                "width": dock.width,
            })
        };
        json!({ "left": dock(&self.left), "right": dock(&self.right) })
    }

    /// Reads a saved layout. Unknown panels are dropped and panels missing
    /// from it (added in a newer version) go where the default puts them.
    pub fn from_json(value: &Value) -> Self {
        let mut docks = Docks::default();
        if !value.is_object() {
            return docks;
        }
        let mut placed = Vec::new();
        for side in [DockSide::Left, DockSide::Right] {
            let saved = &value[side.key()];
            let dock = docks.dock_mut(side);
            dock.panels = saved["panels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|key| PanelKind::from_key(key.as_str()?))
                .filter(|kind| !placed.contains(kind))
                .collect();
            placed.extend(dock.panels.iter().copied());
            dock.active = saved["active"].as_str().and_then(PanelKind::from_key);
            if let Some(open) = saved["open"].as_bool() {
                dock.open = open;
            }
            if let Some(width) = saved["width"].as_f64() {
                dock.width = (width as f32).clamp(side.min_width(), side.max_width());
            }
        }
        let defaults = Docks::default();
        for kind in PanelKind::ALL {
            if !placed.contains(&kind) {
                docks.dock_mut(defaults.side_of(kind)).panels.push(kind);
            }
        }
        docks.left.fix_active();
        docks.right.fix_active();
        docks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_round_trips() {
        let mut docks = Docks::default();
        docks.move_panel(PanelKind::Sessions, DockSide::Right);
        docks.right.width = 400.;
        let restored = Docks::from_json(&docks.to_json());
        assert!(restored.left.panels.is_empty());
        assert_eq!(restored.left.active, None);
        assert_eq!(
            restored.right.panels,
            vec![PanelKind::Files, PanelKind::Diff, PanelKind::Sessions]
        );
        assert_eq!(restored.right.active, Some(PanelKind::Sessions));
        assert_eq!(restored.right.width, 400.);
    }

    #[test]
    fn missing_and_duplicate_panels_are_fixed() {
        let saved = json!({
            "left": { "panels": ["files", "bogus"], "active": "diff", "open": true },
            "right": { "panels": ["files"], "open": false },
        });
        let docks = Docks::from_json(&saved);
        assert_eq!(
            docks.left.panels,
            vec![PanelKind::Files, PanelKind::Sessions]
        );
        assert_eq!(docks.left.active, Some(PanelKind::Files));
        assert_eq!(docks.right.panels, vec![PanelKind::Diff]);
        assert_eq!(docks.right.active, Some(PanelKind::Diff));
    }

    #[test]
    fn moving_the_last_panel_empties_the_dock() {
        let mut docks = Docks::default();
        docks.move_panel(PanelKind::Sessions, DockSide::Right);
        assert!(!docks.left.is_shown());
        assert!(docks.is_visible(PanelKind::Sessions));
        docks.move_panel(PanelKind::Sessions, DockSide::Left);
        assert_eq!(docks.right.active, Some(PanelKind::Files));
    }
}
