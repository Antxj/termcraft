//! Session icons ("Bookmark icon", reference `TFORMICONCHOOSER`): a set of
//! built-in icons in the same spirit as the reference (servers, operating
//! systems, devices, network, cloud…), an optional tint, or a small PNG
//! stored in the session. TermCraft draws its own glyphs (Phosphor).

use super::icon;
use base64::Engine as _;
use egui::Color32;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tc_core::{Session, SessionIcon, SessionKind};

pub struct IconDef {
    pub name: &'static str,
    pub glyph: &'static str,
    pub label: &'static str,
}

const fn def(name: &'static str, glyph: &'static str, label: &'static str) -> IconDef {
    IconDef { name, glyph, label }
}

/// Built-in icons by group, in chooser order.
pub const GROUPS: &[(&str, &[IconDef])] = &[
    (
        "Sessions",
        &[
            def("terminal", icon::TERMINAL_WINDOW, "Terminal"),
            def("console", icon::TERMINAL, "Console"),
            def("plug", icon::PLUGS, "Telnet / raw"),
            def("serial", icon::USB, "Serial"),
            def("desktop", icon::DESKTOP, "Remote desktop"),
            def("folder", icon::FOLDER, "Files"),
            def("globe", icon::GLOBE, "Web"),
            def("code", icon::CODE, "Script"),
        ],
    ),
    (
        "Servers",
        &[
            def("server", icon::HARD_DRIVES, "Server"),
            def("disk", icon::HARD_DRIVE, "Storage"),
            def("database", icon::DATABASE, "Database"),
            def("tower", icon::DESKTOP_TOWER, "Workstation"),
            def("monitor", icon::MONITOR, "Monitor"),
            def("laptop", icon::LAPTOP, "Laptop"),
            def("cpu", icon::CPU, "Hardware"),
            def("circuit", icon::CIRCUITRY, "Embedded"),
        ],
    ),
    (
        "Systems",
        &[
            def("linux", icon::LINUX_LOGO, "Linux"),
            def("windows", icon::WINDOWS_LOGO, "Windows"),
            def("apple", icon::APPLE_LOGO, "macOS"),
            def("android", icon::ANDROID_LOGO, "Android"),
            def("mobile", icon::DEVICE_MOBILE, "Mobile"),
            def("robot", icon::ROBOT, "Automation"),
        ],
    ),
    (
        "Network",
        &[
            def("network", icon::NETWORK, "Network"),
            def("tree", icon::TREE_STRUCTURE, "Switch"),
            def("wifi", icon::WIFI_HIGH, "Wireless"),
            def("broadcast", icon::BROADCAST, "Access point"),
            def("firewall", icon::SHIELD_CHECK, "Firewall"),
            def("fire", icon::FIRE, "Hot"),
            def("printer", icon::PRINTER, "Printer"),
            def("camera", icon::SECURITY_CAMERA, "Camera"),
        ],
    ),
    (
        "Cloud",
        &[
            def("cloud", icon::CLOUD, "Cloud"),
            def("upload", icon::CLOUD_ARROW_UP, "Deploy"),
            def("amazon", icon::AMAZON_LOGO, "Amazon"),
            def("google", icon::GOOGLE_LOGO, "Google"),
            def("github", icon::GITHUB_LOGO, "GitHub"),
            def("gitlab", icon::GITLAB_LOGO, "GitLab"),
            def("package", icon::PACKAGE, "Package"),
            def("cube", icon::CUBE, "Container"),
        ],
    ),
    (
        "Symbols",
        &[
            def("star", icon::STAR, "Star"),
            def("heart", icon::HEART, "Favourite"),
            def("flag", icon::FLAG, "Flag"),
            def("warning", icon::WARNING, "Warning"),
            def("bug", icon::BUG, "Debug"),
            def("lock", icon::LOCK, "Secure"),
            def("key", icon::KEY, "Key"),
            def("gear", icon::GEAR, "Settings"),
            def("wrench", icon::WRENCH, "Maintenance"),
            def("lightning", icon::LIGHTNING, "Fast"),
            def("rocket", icon::ROCKET, "Production"),
            def("flask", icon::FLASK, "Test"),
            def("house", icon::HOUSE, "Home"),
            def("buildings", icon::BUILDINGS, "Office"),
            def("game", icon::GAME_CONTROLLER, "Lab"),
            def("brain", icon::BRAIN, "AI"),
        ],
    ),
];

/// Tints offered in the chooser (`None` = default text colour).
pub const COLORS: &[(&str, Option<&str>)] = &[
    ("Default", None),
    ("Blue", Some("#2f7fd8")),
    ("Green", Some("#2ea043")),
    ("Orange", Some("#e0861e")),
    ("Red", Some("#d73a3a")),
    ("Purple", Some("#8957e5")),
    ("Teal", Some("#1f9e9e")),
    ("Yellow", Some("#c9a400")),
    ("Grey", Some("#7d8590")),
];

pub fn find(name: &str) -> Option<&'static IconDef> {
    GROUPS
        .iter()
        .flat_map(|(_, icons)| icons.iter())
        .find(|d| d.name == name)
}

pub fn kind_glyph(kind: &SessionKind) -> &'static str {
    use SessionKind as K;
    match kind {
        K::LocalShell { .. } | K::Ssh { .. } => icon::TERMINAL_WINDOW,
        K::Telnet { .. } => icon::PLUGS,
        K::Serial { .. } => icon::USB,
        K::Rdp { .. } | K::Vnc { .. } => icon::DESKTOP,
        K::Sftp { .. } | K::Ftp { .. } => icon::FOLDER,
        K::Rsh { .. } => icon::TERMINAL,
        K::File { .. } => icon::FILE,
        K::Browser { .. } => icon::BROWSER,
        K::Mosh { .. } => icon::LIGHTNING,
        K::Wsl { .. } => icon::LINUX_LOGO,
        K::AwsS3 { .. } => icon::CLOUD,
    }
}

/// The glyph for a session: its chosen icon, else its type's icon.
pub fn glyph(session: &Session) -> &'static str {
    session
        .icon
        .as_ref()
        .and_then(|i| find(&i.name))
        .map_or_else(|| kind_glyph(&session.kind), |d| d.glyph)
}

pub fn tint(session: &Session) -> Option<Color32> {
    let [r, g, b] = session.icon.as_ref()?.rgb()?;
    Some(Color32::from_rgb(r, g, b))
}

/// Glyph (tinted) and name as one label.
pub fn label(session: &Session, ui: &egui::Ui) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let text = ui.visuals().text_color();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        glyph(session),
        0.0,
        egui::TextFormat::simple(font.clone(), tint(session).unwrap_or(text)),
    );
    job.append(
        &format!("  {}", session.name),
        0.0,
        egui::TextFormat::simple(font, text),
    );
    job
}

/// Decode and check a custom icon file: PNG, at most 64 KiB and 256 px.
pub fn load_png(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() > SessionIcon::MAX_PNG_BYTES {
        return Err(format!(
            "the icon is too large ({} KiB, at most {} KiB)",
            bytes.len() / 1024,
            SessionIcon::MAX_PNG_BYTES / 1024
        ));
    }
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|e| format!("not a PNG image: {e}"))?;
    if img.width() > 256 || img.height() > 256 {
        return Err("the icon must be at most 256×256 pixels".into());
    }
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Textures of custom PNG icons, decoded once per distinct image and kept
/// in the egui context.
type TextureCache = Arc<Mutex<HashMap<u64, Option<egui::TextureHandle>>>>;

pub fn texture(ctx: &egui::Context, png_b64: &str) -> Option<egui::TextureHandle> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    png_b64.hash(&mut h);
    let key = h.finish();
    let cache: TextureCache = ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<TextureCache>(egui::Id::new("session-icon-textures"))
            .clone()
    });
    let mut cache = cache.lock().expect("texture cache");
    cache
        .entry(key)
        .or_insert_with(|| decode_texture(ctx, key, png_b64))
        .clone()
}

/// Texture of the session's custom PNG, if it has one.
pub fn session_texture(ctx: &egui::Context, session: &Session) -> Option<egui::TextureHandle> {
    texture(ctx, session.icon.as_ref()?.png.as_deref()?)
}

fn decode_texture(ctx: &egui::Context, key: u64, png_b64: &str) -> Option<egui::TextureHandle> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(png_b64)
        .ok()
        .filter(|b| b.len() <= SessionIcon::MAX_PNG_BYTES)?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).ok()?;
    let img = img
        .resize(32, 32, image::imageops::FilterType::Triangle)
        .to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
    Some(ctx.load_texture(
        format!("session-icon-{key:x}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

/// "Please choose an icon".
pub struct IconChooser {
    draft: SessionIcon,
    error: Option<String>,
}

pub enum Choice {
    /// Still open.
    Pending,
    Cancel,
    /// `None` = back to the session type's icon.
    Chosen(Option<SessionIcon>),
}

impl IconChooser {
    pub fn new(current: Option<&SessionIcon>) -> Self {
        Self {
            draft: current.cloned().unwrap_or_default(),
            error: None,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Choice {
        let mut choice = Choice::Pending;
        egui::Window::new("Please choose an icon")
            .id(egui::Id::new("icon-chooser"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                let tint = self.draft.rgb().map(|[r, g, b]| Color32::from_rgb(r, g, b));
                for (group, icons) in GROUPS {
                    ui.label(egui::RichText::new(*group).small().strong());
                    ui.horizontal_wrapped(|ui| {
                        ui.set_max_width(460.0);
                        for d in *icons {
                            let selected = self.draft.png.is_none() && self.draft.name == d.name;
                            let mut text = egui::RichText::new(d.glyph).size(22.0);
                            if let Some(c) = tint {
                                text = text.color(c);
                            }
                            let b = ui
                                .add(
                                    egui::Button::new(text)
                                        .selected(selected)
                                        .min_size(egui::vec2(34.0, 34.0)),
                                )
                                .on_hover_text(d.label);
                            if b.clicked() {
                                self.draft.name = d.name.to_string();
                                self.draft.png = None;
                            }
                        }
                    });
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Colour:");
                    for (label, hex) in COLORS {
                        let selected = self.draft.color.as_deref() == *hex;
                        let swatch = hex
                            .and_then(|h| {
                                SessionIcon {
                                    color: Some(h.to_string()),
                                    ..Default::default()
                                }
                                .rgb()
                            })
                            .map_or(ui.visuals().text_color(), |[r, g, b]| {
                                Color32::from_rgb(r, g, b)
                            });
                        let b = ui
                            .add(
                                egui::Button::new(egui::RichText::new(icon::CIRCLE).color(swatch))
                                    .selected(selected),
                            )
                            .on_hover_text(*label);
                        if b.clicked() {
                            self.draft.color = hex.map(String::from);
                        }
                    }
                });
                ui.separator();
                ui.label(egui::RichText::new("Custom icons").small().strong());
                ui.horizontal(|ui| {
                    if let Some(png) = &self.draft.png
                        && let Some(tex) = texture(ui.ctx(), png)
                    {
                        ui.add(egui::Image::new(&tex).fit_to_exact_size(egui::vec2(32.0, 32.0)));
                    }
                    let pick = ui
                        .button("Select a specific icon for this session")
                        .on_hover_text(
                            "Pick a PNG image (at most 64 KiB, 256×256). It is stored in the \
                             session definition, so it travels with the session when shared.",
                        );
                    if pick.clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("PNG image", &["png"])
                            .pick_file()
                    {
                        match std::fs::read(&path)
                            .map_err(|e| e.to_string())
                            .and_then(|b| load_png(&b))
                        {
                            Ok(png) => {
                                self.draft.png = Some(png);
                                self.error = None;
                            }
                            Err(e) => self.error = Some(e),
                        }
                    }
                });
                if let Some(e) = &self.error {
                    ui.colored_label(ui.visuals().error_fg_color, e);
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        let empty = self.draft.name.is_empty()
                            && self.draft.png.is_none()
                            && self.draft.color.is_none();
                        choice = Choice::Chosen((!empty).then(|| self.draft.clone()));
                    }
                    if ui.button("Default icon").clicked() {
                        choice = Choice::Chosen(None);
                    }
                    if ui.button("Cancel").clicked() {
                        choice = Choice::Cancel;
                    }
                });
            });
        choice
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_found() {
        let mut seen = std::collections::HashSet::new();
        for (_, icons) in GROUPS {
            for d in *icons {
                assert!(seen.insert(d.name), "duplicate {}", d.name);
                assert_eq!(find(d.name).unwrap().glyph, d.glyph);
            }
        }
        for (_, hex) in COLORS.iter().skip(1) {
            let icon = SessionIcon {
                color: hex.map(String::from),
                ..Default::default()
            };
            assert!(icon.rgb().is_some(), "{hex:?}");
        }
    }

    #[test]
    fn session_glyph_falls_back_to_type() {
        let mut s = Session::new("x", SessionKind::default_for(0).unwrap());
        assert_eq!(glyph(&s), icon::TERMINAL_WINDOW);
        s.icon = Some(SessionIcon {
            name: "linux".into(),
            color: Some("#2ea043".into()),
            png: None,
        });
        assert_eq!(glyph(&s), icon::LINUX_LOGO);
        assert_eq!(tint(&s), Some(Color32::from_rgb(0x2e, 0xa0, 0x43)));
        s.icon.as_mut().unwrap().name = "no-such-icon".into();
        assert_eq!(glyph(&s), icon::TERMINAL_WINDOW);
    }

    #[test]
    fn custom_png_is_checked() {
        assert!(load_png(b"not a png").is_err());
        assert!(load_png(&vec![0u8; SessionIcon::MAX_PNG_BYTES + 1]).is_err());
        let mut png = Vec::new();
        image::RgbaImage::new(16, 16)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(load_png(&png).is_ok());
        let mut big = Vec::new();
        image::RgbaImage::new(300, 10)
            .write_to(&mut std::io::Cursor::new(&mut big), image::ImageFormat::Png)
            .unwrap();
        assert!(load_png(&big).is_err());
    }
}
