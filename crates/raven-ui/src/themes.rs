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
