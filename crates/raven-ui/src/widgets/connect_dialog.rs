use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::automation_types::SshAuth;
use raven_core::commands::AppCommand;

/// The "Connect to Server" dialog: collects an SFTP destination and sends
/// `ConnectSftp`. It stays open, showing the error, when the connection
/// fails, and the window closes it once `RemoteConnected` arrives.
pub struct ConnectDialog {
    pub window: adw::Window,
    form: gtk::Box,
    connect_btn: gtk::Button,
    spinner: gtk::Spinner,
    error_label: gtk::Label,
    on_closed: RefCell<Option<Box<dyn Fn()>>>,
}

/// Which authentication rows are shown, in the order of the dropdown.
const AUTH_AGENT: u32 = 0;
const AUTH_PASSWORD: u32 = 1;
const AUTH_KEY: u32 = 2;

impl ConnectDialog {
    pub fn new(
        parent: &adw::ApplicationWindow,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Rc<Self> {
        let window = adw::Window::builder()
            .title("Connect to Server")
            .default_width(460)
            .modal(true)
            .transient_for(parent)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(12);
        content.set_margin_bottom(16);

        let form = gtk::Box::new(gtk::Orientation::Vertical, 12);

        let server_group = adw::PreferencesGroup::new();
        server_group.set_title("Server");
        server_group.set_description(Some("SFTP over SSH"));

        let host_row = adw::EntryRow::new();
        host_row.set_title("Host");
        host_row.set_activates_default(true);
        server_group.add(&host_row);

        let port_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(22.0, 1.0, 65535.0, 1.0, 10.0, 0.0)),
            1.0,
            0,
        );
        port_row.set_title("Port");
        server_group.add(&port_row);

        let user_row = adw::EntryRow::new();
        user_row.set_title("Username");
        user_row.set_text(&std::env::var("USER").unwrap_or_default());
        user_row.set_activates_default(true);
        server_group.add(&user_row);

        let folder_row = adw::EntryRow::new();
        folder_row.set_title("Folder (optional, defaults to the login directory)");
        folder_row.set_activates_default(true);
        server_group.add(&folder_row);
        form.append(&server_group);

        let auth_group = adw::PreferencesGroup::new();
        auth_group.set_title("Authentication");

        let auth_row = adw::ComboRow::new();
        auth_row.set_title("Method");
        auth_row.set_model(Some(&gtk::StringList::new(&[
            "SSH agent",
            "Password",
            "Key file",
        ])));
        auth_row.set_selected(AUTH_AGENT);
        auth_group.add(&auth_row);

        let password_row = adw::PasswordEntryRow::new();
        password_row.set_title("Password");
        password_row.set_activates_default(true);
        password_row.set_visible(false);
        auth_group.add(&password_row);

        let key_row = adw::EntryRow::new();
        key_row.set_title("Key file");
        key_row.set_text(&default_key_path().to_string_lossy());
        key_row.set_activates_default(true);
        key_row.set_visible(false);
        auth_group.add(&key_row);

        let passphrase_row = adw::PasswordEntryRow::new();
        passphrase_row.set_title("Passphrase (if the key has one)");
        passphrase_row.set_activates_default(true);
        passphrase_row.set_visible(false);
        auth_group.add(&passphrase_row);
        form.append(&auth_group);

        {
            let password_row = password_row.clone();
            let key_row = key_row.clone();
            let passphrase_row = passphrase_row.clone();
            auth_row.connect_selected_notify(move |row| {
                let method = row.selected();
                password_row.set_visible(method == AUTH_PASSWORD);
                key_row.set_visible(method == AUTH_KEY);
                passphrase_row.set_visible(method == AUTH_KEY);
            });
        }

        content.append(&form);

        let error_label = gtk::Label::new(None);
        error_label.set_halign(gtk::Align::Start);
        error_label.set_xalign(0.0);
        error_label.set_wrap(true);
        error_label.add_css_class("error");
        error_label.set_visible(false);
        content.append(&error_label);

        let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        btn_box.set_halign(gtk::Align::End);

        let spinner = gtk::Spinner::new();
        spinner.set_valign(gtk::Align::Center);
        btn_box.append(&spinner);

        let cancel_btn = gtk::Button::with_label("Cancel");
        {
            let window = window.clone();
            cancel_btn.connect_clicked(move |_| window.close());
        }
        btn_box.append(&cancel_btn);

        let connect_btn = gtk::Button::with_label("Connect");
        connect_btn.add_css_class("suggested-action");
        btn_box.append(&connect_btn);
        content.append(&btn_box);

        toolbar_view.set_content(Some(&content));
        window.set_content(Some(&toolbar_view));
        window.set_default_widget(Some(&connect_btn));

        let dialog = Rc::new(Self {
            window,
            form,
            connect_btn,
            spinner,
            error_label,
            on_closed: RefCell::new(None),
        });

        {
            let dialog = dialog.clone();
            let host_row = host_row.clone();
            let user_row = user_row.clone();
            dialog.connect_btn.clone().connect_clicked(move |_| {
                let target = match parse_target(
                    &host_row.text(),
                    port_row.value() as u16,
                    &user_row.text(),
                ) {
                    Ok(target) => target,
                    Err(reason) => {
                        dialog.show_error(reason);
                        return;
                    }
                };

                let auth = match auth_row.selected() {
                    AUTH_PASSWORD => SshAuth::Password {
                        password: password_row.text().to_string(),
                    },
                    AUTH_KEY => {
                        let path = key_row.text().trim().to_string();
                        if path.is_empty() {
                            dialog.show_error("A key file is required");
                            return;
                        }
                        let passphrase = passphrase_row.text().to_string();
                        SshAuth::KeyFile {
                            path: expand_home(&path),
                            passphrase: (!passphrase.is_empty()).then_some(passphrase),
                        }
                    }
                    _ => SshAuth::Agent,
                };

                let folder = folder_row.text().trim().to_string();
                dialog.set_busy(true);
                let _ = command_tx.send(AppCommand::ConnectSftp {
                    host: target.host,
                    port: target.port,
                    user: target.user,
                    auth,
                    remote_path: (!folder.is_empty()).then_some(folder),
                });
            });
        }

        {
            let dialog = dialog.clone();
            dialog.window.clone().connect_close_request(move |_| {
                if let Some(cb) = dialog.on_closed.borrow().as_ref() {
                    cb();
                }
                glib::Propagation::Proceed
            });
        }
        {
            let window = dialog.window.clone();
            let key_controller = gtk::EventControllerKey::new();
            key_controller.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape {
                    window.close();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            dialog.window.add_controller(key_controller);
        }

        host_row.grab_focus();
        dialog
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn close(&self) {
        self.window.close();
    }

    /// Run `cb` when the dialog is closed by any route.
    pub fn connect_closed(&self, cb: impl Fn() + 'static) {
        *self.on_closed.borrow_mut() = Some(Box::new(cb));
    }

    /// The connection failed: show why and let the user try again.
    pub fn show_error(&self, message: &str) {
        self.set_busy(false);
        self.error_label.set_text(message);
        self.error_label.set_visible(true);
    }

    fn set_busy(&self, busy: bool) {
        self.form.set_sensitive(!busy);
        self.connect_btn.set_sensitive(!busy);
        self.spinner.set_spinning(busy);
        if busy {
            self.error_label.set_visible(false);
        }
    }
}

/// Where the connection goes, after the host field has been read.
#[derive(Debug, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
    user: String,
}

/// Read the host field, allowing `user@host:port` and an `sftp://` prefix
/// as shorthand for filling the other fields. Anything spelled out in the
/// host field wins over the separate rows.
fn parse_target(host_field: &str, port: u16, user_field: &str) -> Result<Target, &'static str> {
    let mut rest = host_field.trim();
    if let Some(stripped) = rest.strip_prefix("sftp://") {
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix("ssh://") {
        rest = stripped;
    }
    let rest = rest.trim_end_matches('/');

    let (user, rest) = match rest.rsplit_once('@') {
        Some((u, h)) if !u.is_empty() => (u.to_string(), h),
        _ => (user_field.trim().to_string(), rest),
    };
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => match p.parse::<u16>() {
            Ok(p) if p > 0 => (h.to_string(), p),
            _ => return Err("The port must be a number between 1 and 65535"),
        },
        _ => (rest.to_string(), port),
    };

    if host.is_empty() {
        return Err("A host is required");
    }
    if user.is_empty() {
        return Err("A username is required");
    }
    Ok(Target { host, port, user })
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home_dir().join(rest),
        None => PathBuf::from(path),
    }
}

/// The first of the usual key files that exists, else the modern default.
fn default_key_path() -> PathBuf {
    let ssh_dir = home_dir().join(".ssh");
    for name in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        let candidate = ssh_dir.join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    ssh_dir.join("id_ed25519")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_host_uses_the_other_fields() {
        assert_eq!(
            parse_target("example.org", 22, "alice"),
            Ok(Target {
                host: "example.org".into(),
                port: 22,
                user: "alice".into()
            })
        );
    }

    #[test]
    fn shorthand_in_the_host_field_wins() {
        assert_eq!(
            parse_target("sftp://bob@example.org:2222/", 22, "alice"),
            Ok(Target {
                host: "example.org".into(),
                port: 2222,
                user: "bob".into()
            })
        );
    }

    #[test]
    fn missing_pieces_are_reported() {
        assert!(parse_target("", 22, "alice").is_err());
        assert!(parse_target("example.org", 22, "").is_err());
        assert!(parse_target("example.org:notaport", 22, "alice").is_err());
    }

    #[test]
    fn tilde_expands_to_home() {
        let expanded = expand_home("~/.ssh/id_ed25519");
        assert!(expanded.ends_with(".ssh/id_ed25519"));
        assert!(!expanded.to_string_lossy().starts_with('~'));
        assert_eq!(expand_home("/abs/key"), PathBuf::from("/abs/key"));
    }
}
