//! The icon in the system tray (notification area / menu bar / status
//! notifier) and its menu, as in the original: Show Terminal, Show popup
//! terminal, Sessions, Start X server, Text editor, Documentation, About,
//! Exit. "Iconify TermCraft" hides the window here; clicking the icon (or
//! Show Terminal) brings it back.
//!
//! Windows and macOS use the native tray; Linux the StatusNotifierItem
//! protocol (KDE, XFCE, Cinnamon…; GNOME needs the AppIndicator extension).
//! Without a tray, Iconify minimizes the window instead.

use crate::ui::Action;
use crossbeam_channel::Receiver;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};
use tc_commands::Command;
use tc_core::Session;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

/// What the tray asks the window to do.
pub enum Request {
    /// Bring the window back (and to the front).
    Show,
    /// Open the popup terminal (the window stays as it is).
    PopupTerminal,
    /// Show the window, then run this.
    Then(Action),
}

enum Event {
    Icon(TrayIconEvent),
    Menu(MenuEvent),
}

pub struct Tray {
    icon: TrayIcon,
    events: Receiver<Event>,
    /// Fingerprint of the session list in the menu.
    sessions: u64,
    checked: Instant,
}

const SHOW: &str = "show";
const POPUP: &str = "popup";
const NEW_SESSION: &str = "new_session";
const MANAGE: &str = "manage_sessions";
const EDITOR: &str = "editor";
const ABOUT: &str = "about";
const EXIT: &str = "exit";
const X_SERVER: &str = "x_server";
const DOCS: &str = "documentation";
/// Prefix of the menu ids of saved sessions (followed by the session id).
const SESSION: &str = "session:";

impl Tray {
    /// Add the icon to the tray; `None` when the system has no tray.
    pub fn new(ctx: &egui::Context, sessions: &[Session]) -> Option<Self> {
        let (tx, events) = crossbeam_channel::unbounded();
        // The handlers run on the platform's thread even while the window is
        // hidden: the repaint wakes the app up to handle them.
        {
            let (tx, ctx) = (tx.clone(), ctx.clone());
            TrayIconEvent::set_event_handler(Some(move |e| {
                let _ = tx.send(Event::Icon(e));
                ctx.request_repaint();
            }));
        }
        {
            let ctx = ctx.clone();
            MenuEvent::set_event_handler(Some(move |e| {
                let _ = tx.send(Event::Menu(e));
                ctx.request_repaint();
            }));
        }
        let egui::IconData {
            rgba,
            width,
            height,
        } = crate::app_icon();
        let image = tray_icon::Icon::from_rgba(rgba, width, height).ok()?;
        let icon = TrayIconBuilder::new()
            .with_icon(image)
            .with_tooltip("TermCraft")
            .with_menu(Box::new(menu(sessions)))
            // macOS shows the menu on a click, like its other menu bar
            // extras; elsewhere a click brings the window back.
            .with_menu_on_left_click(cfg!(target_os = "macos"))
            .build()
            .ok()?;
        Some(Self {
            icon,
            events,
            sessions: fingerprint(sessions),
            checked: Instant::now(),
        })
    }

    /// Keep the Sessions submenu up to date (checked once a second).
    pub fn sync_sessions(&mut self, sessions: &[Session]) {
        if self.checked.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.checked = Instant::now();
        let now = fingerprint(sessions);
        if now != self.sessions {
            self.sessions = now;
            self.icon.set_menu(Some(Box::new(menu(sessions))));
        }
    }

    /// Requests since the last frame.
    pub fn take(&self) -> Vec<Request> {
        self.events
            .try_iter()
            .filter_map(|e| match e {
                Event::Icon(e) => icon_request(&e),
                Event::Menu(e) => menu_request(e.id.as_ref()),
            })
            .collect()
    }
}

fn icon_request(e: &TrayIconEvent) -> Option<Request> {
    match e {
        TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        }
        | TrayIconEvent::DoubleClick {
            button: MouseButton::Left,
            ..
        } if !cfg!(target_os = "macos") => Some(Request::Show),
        _ => None,
    }
}

/// The request for a menu item id.
pub fn menu_request(id: &str) -> Option<Request> {
    Some(match id {
        SHOW => Request::Show,
        POPUP => Request::PopupTerminal,
        NEW_SESSION => Request::Then(Action::NewSession),
        MANAGE => Request::Then(Action::SidebarTab(tc_core::settings::SidebarTab::Sessions)),
        EDITOR => Request::Then(Action::TextEditor),
        ABOUT => Request::Then(Action::About),
        X_SERVER => Request::Then(Action::Run(Command::XServerStart {})),
        DOCS => Request::Then(Action::Documentation),
        EXIT => Request::Then(Action::Quit),
        _ => {
            let id = id.strip_prefix(SESSION)?;
            Request::Then(Action::Run(Command::TabOpen {
                session: Some(id.to_string()),
            }))
        }
    })
}

fn fingerprint(sessions: &[Session]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for s in sessions {
        (s.id.to_string(), &s.name, &s.folder).hash(&mut h);
    }
    h.finish()
}

fn item(id: &str, text: &str) -> MenuItem {
    MenuItem::with_id(id, text, true, None)
}

/// Sessions grouped by folder (`a/b` → submenu a › submenu b).
#[derive(Default)]
struct Folder<'a> {
    folders: BTreeMap<&'a str, Folder<'a>>,
    sessions: Vec<&'a Session>,
}

impl<'a> Folder<'a> {
    fn tree(sessions: &'a [Session]) -> Self {
        let mut root = Folder::default();
        for s in sessions {
            let mut f = &mut root;
            for part in s.folder.split('/').filter(|p| !p.is_empty()) {
                f = f.folders.entry(part).or_default();
            }
            f.sessions.push(s);
        }
        root
    }

    fn append_to(&self, menu: &Submenu) {
        for (name, sub) in &self.folders {
            let m = Submenu::new(name, true);
            sub.append_to(&m);
            let _ = menu.append(&m);
        }
        for s in &self.sessions {
            let _ = menu.append(&item(&format!("{SESSION}{}", s.id), &s.name));
        }
    }
}

fn menu(sessions: &[Session]) -> Menu {
    let sessions_menu = Submenu::new("Sessions", true);
    let _ = sessions_menu.append(&item(NEW_SESSION, "New Session"));
    let _ = sessions_menu.append(&item(MANAGE, "Manage my sessions"));
    if !sessions.is_empty() {
        let _ = sessions_menu.append(&PredefinedMenuItem::separator());
        Folder::tree(sessions).append_to(&sessions_menu);
    }
    let menu = Menu::new();
    let _ = menu.append_items(&[
        &item(SHOW, "Show Terminal"),
        &item(POPUP, "Show popup terminal"),
        &PredefinedMenuItem::separator(),
        &sessions_menu,
        &PredefinedMenuItem::separator(),
        &item(X_SERVER, "Start X server"),
        &item(EDITOR, "Text editor"),
        &PredefinedMenuItem::separator(),
        &item(DOCS, "Documentation"),
        &item(ABOUT, "About TermCraft"),
        &PredefinedMenuItem::separator(),
        &item(EXIT, "Exit TermCraft"),
    ]);
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_requests() {
        assert!(matches!(menu_request(SHOW), Some(Request::Show)));
        assert!(matches!(menu_request(POPUP), Some(Request::PopupTerminal)));
        assert!(matches!(
            menu_request(EXIT),
            Some(Request::Then(Action::Quit))
        ));
        let id = "67e55044-10b1-426f-9247-bb680e5fe0c8";
        assert!(matches!(
            menu_request(&format!("{SESSION}{id}")),
            Some(Request::Then(Action::Run(Command::TabOpen { session: Some(s) }))) if s == id
        ));
        assert!(menu_request("something else").is_none());
    }

    #[test]
    fn the_icon_decodes() {
        let icon = crate::app_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4);
        assert!(tray_icon::Icon::from_rgba(icon.rgba, icon.width, icon.height).is_ok());
    }

    #[test]
    fn sessions_are_grouped_by_folder() {
        let mk = |name: &str, folder: &str| {
            let mut s = Session::new(name, tc_core::SessionKind::default_for(0).unwrap());
            s.folder = folder.into();
            s
        };
        let list = [
            mk("a", ""),
            mk("b", "prod/web"),
            mk("c", "prod"),
            mk("d", "prod/web"),
        ];
        let tree = Folder::tree(&list);
        assert_eq!(tree.sessions.len(), 1);
        let prod = &tree.folders["prod"];
        assert_eq!(prod.sessions[0].name, "c");
        assert_eq!(prod.folders["web"].sessions.len(), 2);
        assert_ne!(fingerprint(&list), fingerprint(&list[..3]));
    }
}
