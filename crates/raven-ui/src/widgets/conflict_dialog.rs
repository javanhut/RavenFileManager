use std::cell::Cell;
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::operations::{ConflictInfo, ConflictStrategy};

use crate::widgets::file_list::format_size;

/// Ask what to do about a file that already exists at the destination.
///
/// The operation that hit the conflict is blocked in the backend until this
/// dialog answers with `ResolveConflict`, so every way out of it answers:
/// the three choices, cancelling the whole operation, or closing the window,
/// which counts as Skip.
pub fn show_conflict_dialog(
    parent: &adw::ApplicationWindow,
    conflict: &ConflictInfo,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) -> adw::Window {
    let id = conflict.operation_id;
    let name = conflict
        .destination
        .file_name()
        .unwrap_or("this file")
        .to_string();
    let dest_dir = conflict
        .destination
        .parent()
        .map(|p| p.to_string())
        .unwrap_or_default();

    // Overwriting a folder with a folder merges: its other contents stay and
    // only same-named files inside are replaced. Say so, rather than
    // promising to replace "a file".
    let merge = conflict.source_is_dir && conflict.dest_is_dir;
    let (title, from_label, existing_label, replace_label) = if merge {
        ("Merge Folders?", "Merge from:", "Existing folder:", "Merge")
    } else if conflict.dest_is_dir {
        ("Replace Folder?", "Replace with:", "Existing folder:", "Replace")
    } else {
        ("Replace File?", "Replace with:", "Existing file:", "Replace")
    };

    let dialog = adw::Window::builder()
        .title(title)
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

    let heading = gtk::Label::new(Some(&format!(
        "\u{201c}{}\u{201d} already exists in \u{201c}{}\u{201d}",
        name, dest_dir
    )));
    heading.add_css_class("title-4");
    heading.set_halign(gtk::Align::Start);
    heading.set_wrap(true);
    heading.set_xalign(0.0);
    content.append(&heading);

    if merge {
        let note = gtk::Label::new(Some(
            "Merging keeps everything already in the folder. Files inside it that have the same name as incoming files are replaced without asking again.",
        ));
        note.set_halign(gtk::Align::Start);
        note.set_xalign(0.0);
        note.set_wrap(true);
        content.append(&note);
    }

    let details = gtk::Grid::new();
    details.set_row_spacing(4);
    details.set_column_spacing(12);
    for (row, (what, path, size, is_dir)) in [
        (from_label, &conflict.source, conflict.source_size, conflict.source_is_dir),
        (existing_label, &conflict.destination, conflict.dest_size, conflict.dest_is_dir),
    ]
    .into_iter()
    .enumerate()
    {
        let what_label = gtk::Label::new(Some(what));
        what_label.add_css_class("dim-label");
        what_label.set_halign(gtk::Align::Start);
        what_label.set_valign(gtk::Align::Start);
        details.attach(&what_label, 0, row as i32, 1, 1);

        // A folder's own size is not the size of what is in it.
        let text = if is_dir {
            path.to_string()
        } else {
            format!("{} ({})", path, format_size(size))
        };
        let path_label = gtk::Label::new(Some(&text));
        path_label.set_halign(gtk::Align::Start);
        path_label.set_xalign(0.0);
        path_label.set_hexpand(true);
        path_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        path_label.set_selectable(true);
        details.attach(&path_label, 1, row as i32, 1, 1);
    }
    content.append(&details);

    let apply_all = gtk::CheckButton::with_label("Apply this choice to the remaining conflicts");
    content.append(&apply_all);

    let answered = Rc::new(Cell::new(false));

    let answer = {
        let dialog = dialog.clone();
        let cmd_tx = command_tx.clone();
        let answered = answered.clone();
        let apply_all = apply_all.clone();
        Rc::new(move |strategy: ConflictStrategy| {
            if answered.replace(true) {
                return;
            }
            let strategy = if apply_all.is_active() {
                match strategy {
                    ConflictStrategy::Skip => ConflictStrategy::SkipAll,
                    ConflictStrategy::Overwrite => ConflictStrategy::OverwriteAll,
                    ConflictStrategy::Rename => ConflictStrategy::RenameAll,
                    other => other,
                }
            } else {
                strategy
            };
            let _ = cmd_tx.send(AppCommand::ResolveConflict { id, strategy });
            dialog.close();
        })
    };

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_box.set_margin_top(8);

    let cancel_btn = gtk::Button::with_label("Cancel Operation");
    {
        let dialog = dialog.clone();
        let cmd_tx = command_tx.clone();
        let answered = answered.clone();
        cancel_btn.connect_clicked(move |_| {
            // The executor notices the cancel while it waits, so no answer is
            // needed; marking it answered keeps the close handler quiet.
            answered.set(true);
            let _ = cmd_tx.send(AppCommand::CancelOperation { id });
            dialog.close();
        });
    }
    btn_box.append(&cancel_btn);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    btn_box.append(&spacer);

    for (label, strategy, class) in [
        ("Skip", ConflictStrategy::Skip, None),
        ("Rename", ConflictStrategy::Rename, Some("suggested-action")),
        (replace_label, ConflictStrategy::Overwrite, Some("destructive-action")),
    ] {
        let btn = gtk::Button::with_label(label);
        if let Some(class) = class {
            btn.add_css_class(class);
        }
        let answer = answer.clone();
        btn.connect_clicked(move |_| answer(strategy));
        btn_box.append(&btn);
    }
    content.append(&btn_box);

    // Closing the window with nothing chosen must not leave the operation
    // blocked forever.
    {
        let answer = answer.clone();
        dialog.connect_close_request(move |_| {
            answer(ConflictStrategy::Skip);
            glib::Propagation::Proceed
        });
    }
    {
        let dialog_for_key = dialog.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                dialog_for_key.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        dialog.add_controller(key_controller);
    }

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
    dialog
}
