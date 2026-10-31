//! The tray icon's menu as data (status, the pairing code, paired devices
//! with Forget, Quit), so it is unit-tested on every OS; `tray` serves it
//! over `com.canonical.dbusmenu`.

/// What the tray shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrayState {
    /// One line: listening, streaming to whom, or an error.
    pub status: String,
    /// The pairing code while a phone pairs ("123 456").
    pub pairing: Option<(String, String)>,
    /// Paired phones: (device id, name).
    pub devices: Vec<(String, String)>,
    pub streaming: bool,
}

/// What a click does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    Forget { device_id: String, name: String },
    Quit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    /// Stable for the same state; 0 is the root.
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub separator: bool,
    pub children: Vec<MenuItem>,
    pub action: Option<MenuAction>,
}

impl MenuItem {
    fn label(id: i32, label: impl Into<String>) -> Self {
        MenuItem { id, label: label.into(), enabled: false, separator: false, children: Vec::new(), action: None }
    }

    fn action(id: i32, label: impl Into<String>, action: MenuAction) -> Self {
        MenuItem { enabled: true, action: Some(action), ..MenuItem::label(id, label) }
    }

    fn separator(id: i32) -> Self {
        MenuItem { separator: true, ..MenuItem::label(id, "") }
    }

    /// This item and its descendants, depth first.
    pub fn walk(&self) -> Vec<&MenuItem> {
        let mut all = vec![self];
        for child in &self.children {
            all.extend(child.walk());
        }
        all
    }

    pub fn find(&self, id: i32) -> Option<&MenuItem> {
        self.walk().into_iter().find(|item| item.id == id)
    }
}

/// Six digits shown in two groups, as on the Mac.
pub fn format_code(code: &str) -> String {
    if code.len() == 6 && code.is_ascii() { format!("{} {}", &code[..3], &code[3..]) } else { code.to_owned() }
}

pub fn build_menu(state: &TrayState) -> MenuItem {
    let mut items = vec![MenuItem::label(1, state.status.clone())];
    if let Some((device, code)) = &state.pairing {
        items.push(MenuItem::label(2, format!("Pairing code for {device}: {}", format_code(code))));
    }
    items.push(MenuItem::separator(3));
    let mut devices = MenuItem::label(4, "Paired devices");
    devices.enabled = true;
    if state.devices.is_empty() {
        devices.children.push(MenuItem::label(100, "None yet: pick this computer in the phone app"));
    }
    for (i, (device_id, name)) in state.devices.iter().enumerate() {
        let action = MenuAction::Forget { device_id: device_id.clone(), name: name.clone() };
        devices.children.push(MenuItem::action(101 + i as i32, format!("Forget “{name}”"), action));
    }
    items.push(devices);
    items.push(MenuItem::separator(5));
    items.push(MenuItem::action(6, "Quit Sidecast", MenuAction::Quit));
    MenuItem { children: items, ..MenuItem::label(0, "") }
}

/// The tooltip / title line.
pub fn tooltip(state: &TrayState) -> String {
    match &state.pairing {
        Some((device, code)) => format!("Pairing with {device}: enter {} on the phone", format_code(code)),
        None => state.status.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_lists_status_devices_and_quit() {
        let state = TrayState {
            status: "Streaming to Pixel 8".into(),
            pairing: None,
            devices: vec![("a".into(), "Pixel 8".into()), ("b".into(), "iPad".into())],
            streaming: true,
        };
        let root = build_menu(&state);
        let labels: Vec<&str> = root.children.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, ["Streaming to Pixel 8", "", "Paired devices", "", "Quit Sidecast"]);
        let devices = root.find(4).unwrap();
        assert_eq!(devices.children.len(), 2);
        assert_eq!(devices.children[1].label, "Forget “iPad”");
        assert_eq!(
            root.find(102).unwrap().action,
            Some(MenuAction::Forget { device_id: "b".into(), name: "iPad".into() })
        );
        assert_eq!(root.find(6).unwrap().action, Some(MenuAction::Quit));
        assert!(!root.find(1).unwrap().enabled, "the status line is not clickable");
        assert!(root.find(3).unwrap().separator);
        // Ids are unique.
        let mut ids: Vec<i32> = root.walk().iter().map(|i| i.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), root.walk().len());
    }

    #[test]
    fn pairing_code_is_shown_while_pairing() {
        let state = TrayState {
            status: "Waiting for a phone".into(),
            pairing: Some(("Pixel 8".into(), "123456".into())),
            ..TrayState::default()
        };
        let root = build_menu(&state);
        assert_eq!(root.find(2).unwrap().label, "Pairing code for Pixel 8: 123 456");
        assert_eq!(root.find(100).unwrap().label, "None yet: pick this computer in the phone app");
        assert_eq!(tooltip(&state), "Pairing with Pixel 8: enter 123 456 on the phone");
        assert_eq!(format_code("12345"), "12345");
    }
}
