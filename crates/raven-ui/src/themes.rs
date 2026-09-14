//! The look: Raven Glass, the stylesheet shared by the Raven apps
//! (`data/resources/raven-glass.css`, kept byte-identical with the copies in
//! Raven Settings, Store, Power and Viewer), then the file manager's own
//! classes (`data/resources/style.css`).
//!
//! Three providers, lowest first:
//! - `APPLICATION`: Raven Glass, then the file manager's classes.
//! - `APPLICATION + 1`: the theme -- accent, the light sheet when light, and a
//!   palette theme's surface colours. Replaced whenever the theme or the
//!   desktop's appearance changes.
//! - `APPLICATION + 2`: font and icon sizes ([`apply_sizes`]).
//!
//! Windows carry the `raven` class the shared sheet keys on, and `glass` while
//! the desktop has transparency on. Both are put on every toplevel the process
//! creates, as it is created, so no dialog can be missed.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;

use raven_core::config::{DesktopAppearance, DesktopThemeMode, Theme};

const RAVEN_GLASS_CSS: &str = include_str!("../../../data/resources/raven-glass.css");
const RAVEN_GLASS_LIGHT_CSS: &str = include_str!("../../../data/resources/raven-glass-light.css");
const APP_CSS: &str = include_str!("../../../data/resources/style.css");

/// How long `desktop.toml` has to be quiet before it is read again: Settings
/// writes it in several steps, and one re-read per change is enough.
const DESKTOP_SETTLE: Duration = Duration::from_millis(150);

/// Light, dark, or the desktop's "auto".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheme {
    Dark,
    Light,
    /// libadwaita prefers dark and the surfaces stay dark Raven Glass; the
    /// light sheet is only for an explicit "light", as in Raven Store and
    /// Raven Settings.
    System,
}

impl Scheme {
    fn color_scheme(self) -> adw::ColorScheme {
        match self {
            Scheme::Dark => adw::ColorScheme::ForceDark,
            Scheme::Light => adw::ColorScheme::ForceLight,
            // Dark unless the system asks for light, as the other Raven apps
            // read "auto".
            Scheme::System => adw::ColorScheme::PreferDark,
        }
    }
}

/// The surfaces of a palette theme. Only colours: radii, hairlines and type
/// stay Raven Glass whatever the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub window: &'static str,
    pub foreground: &'static str,
    pub sidebar: &'static str,
    pub dialog: &'static str,
    pub popover: &'static str,
}

/// What a theme comes to once the desktop's appearance is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Look {
    pub scheme: Scheme,
    pub accent: String,
    /// `None` keeps Raven Glass's own surfaces.
    pub palette: Option<Palette>,
}

impl Look {
    pub fn resolve(theme: Theme, desktop: &DesktopAppearance) -> Self {
        let fixed = |light: bool, accent: &str, palette: Option<Palette>| Look {
            scheme: if light { Scheme::Light } else { Scheme::Dark },
            accent: accent.to_string(),
            palette,
        };
        match theme {
            Theme::Raven => Look {
                scheme: match desktop.theme_mode {
                    DesktopThemeMode::Dark => Scheme::Dark,
                    DesktopThemeMode::Light => Scheme::Light,
                    DesktopThemeMode::Auto => Scheme::System,
                },
                accent: desktop.accent.clone(),
                palette: None,
            },
            Theme::AdwaitaDark => fixed(false, "#3584e4", None),
            Theme::AdwaitaLight => fixed(true, "#3584e4", None),
            Theme::CatppuccinMocha => fixed(
                false,
                "#cba6f7",
                Some(Palette {
                    window: "#1e1e2e",
                    foreground: "#cdd6f4",
                    sidebar: "#181825",
                    dialog: "#24243a",
                    popover: "#313244",
                }),
            ),
            Theme::CatppuccinLatte => fixed(
                true,
                "#8839ef",
                Some(Palette {
                    window: "#eff1f5",
                    foreground: "#4c4f69",
                    sidebar: "#e6e9ef",
                    dialog: "#f5f6f9",
                    popover: "#ffffff",
                }),
            ),
            Theme::Nord => fixed(
                false,
                "#88c0d0",
                Some(Palette {
                    window: "#2e3440",
                    foreground: "#eceff4",
                    sidebar: "#292e39",
                    dialog: "#3b4252",
                    popover: "#434c5e",
                }),
            ),
            Theme::Dracula => fixed(
                false,
                "#bd93f9",
                Some(Palette {
                    window: "#282a36",
                    foreground: "#f8f8f2",
                    sidebar: "#21222c",
                    dialog: "#2f3140",
                    popover: "#44475a",
                }),
            ),
            Theme::Frost => fixed(
                false,
                "#38bdf8",
                Some(Palette {
                    window: "#0f172a",
                    foreground: "#e2e8f0",
                    sidebar: "#0b1222",
                    dialog: "#152036",
                    popover: "#1e2a44",
                }),
            ),
            Theme::RosePine => fixed(
                false,
                "#c4a7e7",
                Some(Palette {
                    window: "#191724",
                    foreground: "#e0def4",
                    sidebar: "#1f1d2e",
                    dialog: "#1f1d2e",
                    popover: "#26233a",
                }),
            ),
        }
    }

    /// The theme provider's stylesheet, for a scheme that came out `light`.
    pub fn css(&self, light: bool) -> String {
        let mut css = format!(
            "@define-color accent_bg_color {0};\n@define-color accent_color {0};\n",
            self.accent
        );
        if light {
            css.push_str(RAVEN_GLASS_LIGHT_CSS);
            // The light sheet sits a provider above the file manager's own
            // rules and would win over them for the widgets both style; the
            // rules are written against the foreground colour, so repeating
            // them here is all light mode needs.
            css.push_str(APP_CSS);
        }
        if let Some(p) = self.palette {
            css.push_str(&palette_css(&p, light));
        }
        css
    }
}

fn palette_css(p: &Palette, light: bool) -> String {
    let glass = if light { 0.88 } else { 0.85 };
    format!(
        "@define-color window_bg_color {w};\n\
         @define-color window_fg_color {f};\n\
         @define-color headerbar_bg_color {w};\n\
         @define-color headerbar_fg_color {f};\n\
         @define-color view_bg_color {w};\n\
         @define-color view_fg_color {f};\n\
         @define-color card_fg_color {f};\n\
         @define-color dialog_bg_color {d};\n\
         @define-color dialog_fg_color {f};\n\
         @define-color popover_bg_color {o};\n\
         @define-color popover_fg_color {f};\n\
         @define-color sidebar_bg_color {s};\n\
         @define-color sidebar_fg_color {f};\n\
         @define-color sidebar_backdrop_color {s};\n\
         window.raven.glass {{ background-color: alpha({w}, {glass}); }}\n\
         window.raven.glass .sidebar {{ background-color: alpha({s}, 0.55); }}\n",
        w = p.window,
        f = p.foreground,
        s = p.sidebar,
        d = p.dialog,
        o = p.popover,
    )
}

thread_local! {
    static THEME: Cell<Theme> = const { Cell::new(Theme::Raven) };
    /// Whether the desktop has transparency on, as last read.
    static GLASS: Cell<bool> = const { Cell::new(false) };
    static STARTED: Cell<bool> = const { Cell::new(false) };
    /// The theme provider and the stylesheet it holds, so an unchanged sheet
    /// is not reloaded (which restyles every widget).
    static THEME_PROVIDER: RefCell<Option<(gtk::CssProvider, String)>> =
        const { RefCell::new(None) };
    /// Kept for the life of the process; a dropped monitor stops.
    static DESKTOP_MONITOR: RefCell<Option<gio::FileMonitor>> = const { RefCell::new(None) };
}

/// Load the look and start following the desktop's appearance. Needs the
/// display, so call it once the application is running; later calls only
/// switch the theme.
pub fn init(theme: Theme) {
    THEME.with(|t| t.set(theme));
    if STARTED.with(|s| s.replace(true)) {
        refresh();
        return;
    }

    let display = gtk::gdk::Display::default().expect("Could not get default display");
    let base = gtk::CssProvider::new();
    base.load_from_string(&format!("{RAVEN_GLASS_CSS}\n{APP_CSS}"));
    gtk::style_context_add_provider_for_display(
        &display,
        &base,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    refresh();
    decorate_toplevels();
    watch_desktop();
}

/// Switch to `theme`, as picked in the settings.
pub fn apply_theme(theme: Theme) {
    init(theme);
}

/// Re-read the desktop's appearance and apply the current theme on top of it.
fn refresh() {
    let desktop = DesktopAppearance::load();
    let look = Look::resolve(THEME.with(Cell::get), &desktop);
    let manager = adw::StyleManager::default();
    // libadwaita's own switch, not GtkSettings' prefer-dark, which it warns
    // about and ignores.
    manager.set_color_scheme(look.scheme.color_scheme());
    // The light sheet only for an explicit "light". Raven Store and Raven
    // Settings read "auto" the same way (prefer dark, dark Raven Glass), and
    // the apps have to agree or one desktop shows a light file manager beside
    // a dark Store.
    let light = look.scheme == Scheme::Light;
    set_theme_css(look.css(light));

    GLASS.with(|g| g.set(desktop.transparency));
    let toplevels = gtk::Window::toplevels();
    for i in 0..toplevels.n_items() {
        if let Some(window) = toplevels.item(i).and_downcast::<gtk::Window>() {
            decorate(&window);
        }
    }
}

fn set_theme_css(css: String) {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    THEME_PROVIDER.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.as_ref().is_some_and(|(_, current)| *current == css) {
            return;
        }
        if let Some((old, _)) = slot.take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
        *slot = Some((provider, css));
    });
}

/// Give a window the Raven classes. Glass is the material of a window of its
/// own; a dialog over one stays opaque, as in the other Raven apps.
fn decorate(window: &gtk::Window) {
    window.add_css_class("raven");
    if GLASS.with(Cell::get) && window.transient_for().is_none() {
        window.add_css_class("glass");
    } else {
        window.remove_css_class("glass");
    }
}

/// Decorate every toplevel now and each one created from here on.
fn decorate_toplevels() {
    let toplevels = gtk::Window::toplevels();
    for i in 0..toplevels.n_items() {
        if let Some(window) = toplevels.item(i).and_downcast::<gtk::Window>() {
            decorate(&window);
        }
    }
    toplevels.connect_items_changed(|model, position, _removed, added| {
        for i in position..position + added {
            let Some(window) = model.item(i).and_downcast::<gtk::Window>() else {
                continue;
            };
            decorate(&window);
            // A window joins the list while it is still being constructed,
            // before a dialog's transient-for is set; decide on glass again
            // once it has been.
            let weak = window.downgrade();
            glib::idle_add_local_once(move || {
                if let Some(window) = weak.upgrade() {
                    decorate(&window);
                }
            });
        }
    });
}

/// Follow `desktop.toml`, so a change made in Raven Settings shows here at
/// once. The directory is watched rather than the file: the file may not
/// exist yet, and may be replaced by renaming a new one over it.
fn watch_desktop() {
    let path = DesktopAppearance::path();
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return;
    };
    let name = name.to_os_string();
    let monitor = match gio::File::for_path(dir)
        .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
    {
        Ok(monitor) => monitor,
        Err(e) => {
            tracing::debug!("Not following {}: {}", path.display(), e);
            return;
        }
    };
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    monitor.connect_changed(move |_, file, other, event| {
        if matches!(
            event,
            gio::FileMonitorEvent::AttributeChanged
                | gio::FileMonitorEvent::PreUnmount
                | gio::FileMonitorEvent::Unmounted
        ) {
            return;
        }
        let names_desktop = |f: Option<&gio::File>| {
            f.and_then(|f| f.basename())
                .is_some_and(|b| b.as_os_str() == name.as_os_str())
        };
        if !names_desktop(Some(file)) && !names_desktop(other) {
            return;
        }
        if let Some(id) = pending.borrow_mut().take() {
            id.remove();
        }
        let fired = pending.clone();
        let id = glib::timeout_add_local_once(DESKTOP_SETTLE, move || {
            fired.borrow_mut().take();
            refresh();
        });
        *pending.borrow_mut() = Some(id);
    });
    DESKTOP_MONITOR.with(|m| *m.borrow_mut() = Some(monitor));
}

/// Apply the font and icon sizes from the appearance settings.
///
/// Replaces the previous size rules, so it can be called again whenever the
/// settings change. Sizes are in CSS pixels; `icon_size` is the base the
/// list, grid and preview views scale from, with 24 giving the stock look.
pub fn apply_sizes(font_size: u32, icon_size: u32) {
    let display = gtk::gdk::Display::default().expect("Could not get default display");

    thread_local! {
        static SIZE_PROVIDER: std::cell::RefCell<Option<gtk::CssProvider>> =
            const { std::cell::RefCell::new(None) };
    }

    let css = size_css(font_size, icon_size);
    SIZE_PROVIDER.with(|cell| {
        if let Some(old) = cell.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&css);
        // Above the theme so a size choice wins over the base stylesheet's
        // fixed table font size.
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 2,
        );
        *cell.borrow_mut() = Some(provider);
    });
}

/// The three icon sizes derived from the base icon size: list rows, the icon
/// grid, and the preview grid's fallback icon.
pub fn icon_sizes(icon_size: u32) -> (u32, u32, u32) {
    let list = (icon_size * 5 / 6).clamp(16, 32);
    let grid = (icon_size * 8 / 3).clamp(32, 128);
    // The preview grid's fallback sits in the 128px square a thumbnail gets;
    // filling it made a type icon outweigh the photos beside it.
    let preview = (icon_size * 3).clamp(48, 144);
    (list, grid, preview)
}

fn size_css(font_size: u32, icon_size: u32) -> String {
    let font_size = font_size.clamp(6, 48);
    let (list, grid, preview) = icon_sizes(icon_size);
    let mut css = format!(
        "window {{ font-size: {font_size}px; }}\n\
         .data-table {{ font-size: {font_size}px; }}\n\
         .raven-list-icon {{ -gtk-icon-size: {list}px; }}\n\
         .raven-grid-icon {{ -gtk-icon-size: {grid}px; }}\n\
         .raven-preview-icon {{ -gtk-icon-size: {preview}px; }}\n"
    );
    css.push_str(&scaled_font_rules(APP_CSS, font_size));
    css
}

/// The font size style.css's type sizes are written for.
const BASE_FONT_SIZE: f64 = 13.0;

/// Every `font-size: Npx` in `css`, re-emitted under its own selectors and
/// scaled from [`BASE_FONT_SIZE`] to `font_size`.
///
/// A label given its own size stops inheriting the window's, so without this
/// the Appearance font size would only reach the labels style.css leaves
/// alone. Read from the sheet rather than listed here so the two cannot drift.
/// Rounded to whole pixels: at a fractional size an ellipsizing label measures
/// narrower than it draws and cuts its own text short.
fn scaled_font_rules(css: &str, font_size: u32) -> String {
    let mut out = String::new();
    for block in strip_css_comments(css).split('}') {
        let Some((selectors, body)) = block.split_once('{') else {
            continue;
        };
        for declaration in body.split(';') {
            let Some((property, value)) = declaration.split_once(':') else {
                continue;
            };
            if property.trim() != "font-size" {
                continue;
            }
            let Some(px) = value
                .trim()
                .strip_suffix("px")
                .and_then(|v| v.trim().parse::<f64>().ok())
            else {
                continue;
            };
            let scaled = (px * f64::from(font_size) / BASE_FONT_SIZE).round().max(1.0);
            out.push_str(&format!("{} {{ font-size: {scaled}px; }}\n", selectors.trim()));
        }
    }
    out
}

fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = match rest[start + 2..].find("*/") {
            Some(end) => &rest[start + 2 + end + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_icon_size_keeps_original_pixel_sizes() {
        assert_eq!(icon_sizes(24), (20, 64, 72));
    }

    #[test]
    fn icon_sizes_stay_within_bounds() {
        assert_eq!(icon_sizes(16), (16, 42, 48));
        assert_eq!(icon_sizes(64), (32, 128, 144));
    }

    /// Each fixed type size in style.css follows the font-size setting.
    #[test]
    fn app_type_sizes_scale_with_the_font_size() {
        let css = "/* a { font-size: 99px; } */\n.a label,\n.b { color: red; font-size: 12px; }\n.c { font-size: 1em; }\n";
        assert_eq!(scaled_font_rules(css, 13), ".a label,\n.b { font-size: 12px; }\n");
        assert_eq!(scaled_font_rules(css, 26), ".a label,\n.b { font-size: 24px; }\n");
        // Whole pixels only.
        assert_eq!(scaled_font_rules(css, 15), ".a label,\n.b { font-size: 14px; }\n");

        let sizes = size_css(15, 24);
        assert!(sizes.contains(".status-bar label { font-size: 14px; }"));
        assert!(sizes.contains("gridview.file-grid > child label { font-size: 14px; }"));
        assert!(sizes.contains(".sidebar list.navigation-sidebar row label { font-size: 16px; }"));
    }

    #[test]
    fn size_css_names_every_class() {
        let css = size_css(13, 24);
        assert!(css.contains("window { font-size: 13px; }"));
        assert!(css.contains(".data-table { font-size: 13px; }"));
        assert!(css.contains(".raven-list-icon { -gtk-icon-size: 20px; }"));
        assert!(css.contains(".raven-grid-icon { -gtk-icon-size: 64px; }"));
        assert!(css.contains(".raven-preview-icon { -gtk-icon-size: 72px; }"));
    }

    #[test]
    fn raven_follows_the_desktop() {
        let desktop = DesktopAppearance {
            theme_mode: DesktopThemeMode::Light,
            accent: "#F7768E".to_string(),
            transparency: false,
        };
        let look = Look::resolve(Theme::Raven, &desktop);
        assert_eq!(look.scheme, Scheme::Light);
        assert_eq!(look.accent, "#F7768E");
        assert!(look.palette.is_none());
        let auto = DesktopAppearance {
            theme_mode: DesktopThemeMode::Auto,
            ..desktop
        };
        assert_eq!(Look::resolve(Theme::Raven, &auto).scheme, Scheme::System);
    }

    #[test]
    fn palette_themes_ignore_the_desktop_colours() {
        let desktop = DesktopAppearance::default();
        let latte = Look::resolve(Theme::CatppuccinLatte, &desktop);
        assert_eq!(latte.scheme, Scheme::Light);
        assert_eq!(latte.accent, "#8839ef");
        let nord = Look::resolve(Theme::Nord, &desktop);
        assert_eq!(nord.scheme, Scheme::Dark);
    }

    #[test]
    fn theme_css_layers_accent_light_sheet_and_palette() {
        let desktop = DesktopAppearance::default();
        let dark = Look::resolve(Theme::Raven, &desktop).css(false);
        assert!(dark.starts_with("@define-color accent_bg_color #7AA2F7;"));
        assert!(!dark.contains("Raven Glass, light"));

        let light = Look::resolve(Theme::AdwaitaLight, &desktop).css(true);
        let light_sheet = light.find("Raven Glass, light").expect("light sheet");
        let app_rules = light.find("Raven File Manager").expect("app rules repeated");
        assert!(light_sheet < app_rules);

        let nord = Look::resolve(Theme::Nord, &desktop).css(false);
        assert!(nord.contains("@define-color window_bg_color #2e3440;"));
        assert!(nord.contains("window.raven.glass { background-color: alpha(#2e3440, 0.85); }"));
    }

    #[test]
    fn every_theme_resolves() {
        let desktop = DesktopAppearance::default();
        for theme in Theme::ALL {
            let look = Look::resolve(*theme, &desktop);
            assert!(raven_core::config::is_hex_colour(&look.accent), "{theme:?}");
        }
    }
}
