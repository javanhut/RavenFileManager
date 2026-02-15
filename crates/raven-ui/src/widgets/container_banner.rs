use gtk4 as gtk;
use gtk::prelude::*;

/// Dismissable info banner shown when running inside a container (Flatpak/Docker/Podman/systemd-nspawn).
pub struct ContainerBanner {
    pub revealer: gtk::Revealer,
    label: gtk::Label,
}

impl ContainerBanner {
    pub fn new() -> Self {
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        revealer.set_reveal_child(false);

        let banner_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        banner_box.set_margin_start(12);
        banner_box.set_margin_end(12);
        banner_box.set_margin_top(4);
        banner_box.set_margin_bottom(4);
        banner_box.add_css_class("info-banner");

        let icon = gtk::Image::from_icon_name("dialog-information-symbolic");
        icon.set_pixel_size(16);
        banner_box.append(&icon);

        let label = gtk::Label::new(None);
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        banner_box.append(&label);

        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("flat");
        close_btn.add_css_class("circular");
        close_btn.set_tooltip_text(Some("Dismiss"));
        {
            let revealer = revealer.clone();
            close_btn.connect_clicked(move |_| {
                revealer.set_reveal_child(false);
            });
        }
        banner_box.append(&close_btn);

        revealer.set_child(Some(&banner_box));

        Self { revealer, label }
    }

    /// Show the banner with container type information.
    pub fn show_container_info(&self, container_type: &str, app_id: Option<&str>) {
        let msg = match app_id {
            Some(id) => format!("Running inside {} container ({})", container_type, id),
            None => format!("Running inside {} container", container_type),
        };
        self.label.set_text(&msg);
        self.revealer.set_reveal_child(true);
    }
}
