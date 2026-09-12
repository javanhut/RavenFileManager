use gtk4 as gtk;
use libadwaita as adw;

use raven_core::config::Theme;

pub fn is_dark_theme(theme: Theme) -> bool {
    match theme {
        Theme::AdwaitaLight | Theme::CatppuccinLatte => false,
        _ => true,
    }
}

pub fn apply_theme(theme: Theme) {
    let style_manager = adw::StyleManager::default();
    if is_dark_theme(theme) {
        style_manager.set_color_scheme(adw::ColorScheme::ForceDark);
    } else {
        style_manager.set_color_scheme(adw::ColorScheme::ForceLight);
    }

    let css = css_for_theme(theme);
    let display = gtk::gdk::Display::default().expect("Could not get default display");

    // Remove any previously-applied theme provider
    thread_local! {
        static THEME_PROVIDER: std::cell::RefCell<Option<gtk::CssProvider>> =
            const { std::cell::RefCell::new(None) };
    }

    THEME_PROVIDER.with(|cell| {
        if let Some(old) = cell.borrow_mut().take() {
            gtk::style_context_remove_provider_for_display(&display, &old);
        }

        if !css.is_empty() {
            let provider = gtk::CssProvider::new();
            provider.load_from_data(css);
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            *cell.borrow_mut() = Some(provider);
        }
    });
}

pub fn load_base_css() {
    let display = gtk::gdk::Display::default().expect("Could not get default display");
    let provider = gtk::CssProvider::new();
    provider.load_from_data(include_str!("../../../data/resources/style.css"));
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
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
    let preview = (icon_size * 16 / 3).clamp(64, 256);
    (list, grid, preview)
}

fn size_css(font_size: u32, icon_size: u32) -> String {
    let font_size = font_size.clamp(6, 48);
    let (list, grid, preview) = icon_sizes(icon_size);
    format!(
        "window {{ font-size: {font_size}px; }}\n\
         .data-table {{ font-size: {font_size}px; }}\n\
         .raven-list-icon {{ -gtk-icon-size: {list}px; }}\n\
         .raven-grid-icon {{ -gtk-icon-size: {grid}px; }}\n\
         .raven-preview-icon {{ -gtk-icon-size: {preview}px; }}\n"
    )
}

fn css_for_theme(theme: Theme) -> &'static str {
    match theme {
        Theme::AdwaitaDark | Theme::AdwaitaLight => "",
        Theme::CatppuccinMocha => CATPPUCCIN_MOCHA_CSS,
        Theme::CatppuccinLatte => CATPPUCCIN_LATTE_CSS,
        Theme::Nord => NORD_CSS,
        Theme::Dracula => DRACULA_CSS,
        Theme::Frost => FROST_CSS,
        Theme::RosePine => ROSE_PINE_CSS,
    }
}

static CATPPUCCIN_MOCHA_CSS: &str = r#"
@define-color window_bg_color #1e1e2e;
@define-color window_fg_color #cdd6f4;
@define-color headerbar_bg_color #181825;
@define-color headerbar_fg_color #cdd6f4;
@define-color card_bg_color #313244;
@define-color card_fg_color #cdd6f4;
@define-color view_bg_color #1e1e2e;
@define-color view_fg_color #cdd6f4;
@define-color sidebar_bg_color #181825;
@define-color sidebar_fg_color #cdd6f4;
@define-color popover_bg_color #313244;
@define-color popover_fg_color #cdd6f4;
@define-color accent_color #cba6f7;
@define-color accent_bg_color #cba6f7;
@define-color accent_fg_color #1e1e2e;
@define-color borders #45475a;
@define-color warning_color #f9e2af;
@define-color success_color #a6e3a1;
@define-color error_color #f38ba8;
"#;

static CATPPUCCIN_LATTE_CSS: &str = r#"
@define-color window_bg_color #eff1f5;
@define-color window_fg_color #4c4f69;
@define-color headerbar_bg_color #e6e9ef;
@define-color headerbar_fg_color #4c4f69;
@define-color card_bg_color #ccd0da;
@define-color card_fg_color #4c4f69;
@define-color view_bg_color #eff1f5;
@define-color view_fg_color #4c4f69;
@define-color sidebar_bg_color #e6e9ef;
@define-color sidebar_fg_color #4c4f69;
@define-color popover_bg_color #ccd0da;
@define-color popover_fg_color #4c4f69;
@define-color accent_color #8839ef;
@define-color accent_bg_color #8839ef;
@define-color accent_fg_color #eff1f5;
@define-color borders #bcc0cc;
@define-color warning_color #df8e1d;
@define-color success_color #40a02b;
@define-color error_color #d20f39;
"#;

static NORD_CSS: &str = r#"
@define-color window_bg_color #2e3440;
@define-color window_fg_color #eceff4;
@define-color headerbar_bg_color #3b4252;
@define-color headerbar_fg_color #eceff4;
@define-color card_bg_color #3b4252;
@define-color card_fg_color #eceff4;
@define-color view_bg_color #2e3440;
@define-color view_fg_color #eceff4;
@define-color sidebar_bg_color #3b4252;
@define-color sidebar_fg_color #d8dee9;
@define-color popover_bg_color #434c5e;
@define-color popover_fg_color #eceff4;
@define-color accent_color #88c0d0;
@define-color accent_bg_color #88c0d0;
@define-color accent_fg_color #2e3440;
@define-color borders #4c566a;
@define-color warning_color #ebcb8b;
@define-color success_color #a3be8c;
@define-color error_color #bf616a;
"#;

static DRACULA_CSS: &str = r#"
@define-color window_bg_color #282a36;
@define-color window_fg_color #f8f8f2;
@define-color headerbar_bg_color #21222c;
@define-color headerbar_fg_color #f8f8f2;
@define-color card_bg_color #44475a;
@define-color card_fg_color #f8f8f2;
@define-color view_bg_color #282a36;
@define-color view_fg_color #f8f8f2;
@define-color sidebar_bg_color #21222c;
@define-color sidebar_fg_color #f8f8f2;
@define-color popover_bg_color #44475a;
@define-color popover_fg_color #f8f8f2;
@define-color accent_color #bd93f9;
@define-color accent_bg_color #bd93f9;
@define-color accent_fg_color #282a36;
@define-color borders #6272a4;
@define-color warning_color #f1fa8c;
@define-color success_color #50fa7b;
@define-color error_color #ff5555;
"#;

static FROST_CSS: &str = r#"
@define-color window_bg_color rgba(15, 23, 42, 0.95);
@define-color window_fg_color #e2e8f0;
@define-color headerbar_bg_color rgba(15, 23, 42, 0.90);
@define-color headerbar_fg_color #e2e8f0;
@define-color card_bg_color rgba(30, 58, 95, 0.85);
@define-color card_fg_color #e2e8f0;
@define-color view_bg_color rgba(15, 23, 42, 0.92);
@define-color view_fg_color #e2e8f0;
@define-color sidebar_bg_color rgba(15, 23, 42, 0.88);
@define-color sidebar_fg_color #cbd5e1;
@define-color popover_bg_color rgba(30, 58, 95, 0.90);
@define-color popover_fg_color #e2e8f0;
@define-color accent_color #38bdf8;
@define-color accent_bg_color #38bdf8;
@define-color accent_fg_color #0f172a;
@define-color borders rgba(56, 189, 248, 0.3);
@define-color warning_color #fbbf24;
@define-color success_color #34d399;
@define-color error_color #f87171;

headerbar {
    background: rgba(15, 23, 42, 0.85);
    border-bottom: 1px solid rgba(56, 189, 248, 0.2);
}

.navigation-sidebar {
    background: rgba(15, 23, 42, 0.80);
    border-right: 1px solid rgba(56, 189, 248, 0.15);
}

.tab-bar {
    background: rgba(15, 23, 42, 0.80);
    border-bottom: 1px solid rgba(56, 189, 248, 0.2);
}

.search-bar {
    background: rgba(30, 58, 95, 0.70);
    border-bottom: 1px solid rgba(56, 189, 248, 0.2);
}

.preview-panel {
    background: rgba(30, 58, 95, 0.60);
    border-left: 1px solid rgba(56, 189, 248, 0.15);
}
"#;

static ROSE_PINE_CSS: &str = r#"
@define-color window_bg_color #191724;
@define-color window_fg_color #e0def4;
@define-color headerbar_bg_color #1f1d2e;
@define-color headerbar_fg_color #e0def4;
@define-color card_bg_color #1f1d2e;
@define-color card_fg_color #e0def4;
@define-color view_bg_color #191724;
@define-color view_fg_color #e0def4;
@define-color sidebar_bg_color #1f1d2e;
@define-color sidebar_fg_color #908caa;
@define-color popover_bg_color #26233a;
@define-color popover_fg_color #e0def4;
@define-color accent_color #c4a7e7;
@define-color accent_bg_color #c4a7e7;
@define-color accent_fg_color #191724;
@define-color borders #26233a;
@define-color warning_color #f6c177;
@define-color success_color #9ccfd8;
@define-color error_color #eb6f92;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stock_icon_size_keeps_original_pixel_sizes() {
        assert_eq!(icon_sizes(24), (20, 64, 128));
    }

    #[test]
    fn icon_sizes_stay_within_bounds() {
        assert_eq!(icon_sizes(16), (16, 42, 85));
        assert_eq!(icon_sizes(64), (32, 128, 256));
    }

    #[test]
    fn size_css_names_every_class() {
        let css = size_css(13, 24);
        assert!(css.contains("window { font-size: 13px; }"));
        assert!(css.contains(".data-table { font-size: 13px; }"));
        assert!(css.contains(".raven-list-icon { -gtk-icon-size: 20px; }"));
        assert!(css.contains(".raven-grid-icon { -gtk-icon-size: 64px; }"));
        assert!(css.contains(".raven-preview-icon { -gtk-icon-size: 128px; }"));
    }
}
