use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::custom_actions::{self, join_args, split_args, CustomAction, ShowFor};

use crate::state::AppState;

/// The "Actions" settings page: the custom context-menu commands.
pub fn build_actions_page(state: AppState) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(16);
    content.set_margin_bottom(16);

    let group = adw::PreferencesGroup::new();
    group.set_title("Context Menu Actions");
    group.set_description(Some(&format!(
        "Commands offered in the right-click menu. Placeholders: {{path}} the selected item, \
         {{paths}} every selected item, {{directory}} the folder, {{name}} the file name. \
         Saved to {}.",
        custom_actions::user_actions_path().display()
    )));

    let list = Rc::new(gtk::Box::new(gtk::Orientation::Vertical, 8));
    group.add(list.as_ref());

    let render: Rc<dyn Fn()> = {
        let list = list.clone();
        let state = state.clone();
        let render_cell: Rc<std::cell::RefCell<Option<Rc<dyn Fn()>>>> =
            Rc::new(std::cell::RefCell::new(None));
        let render_cell_for_closure = render_cell.clone();
        let render: Rc<dyn Fn()> = Rc::new(move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let actions = state.borrow().custom_actions.clone();
            if actions.is_empty() {
                let empty = gtk::Label::new(Some("No actions. Add one, or restore the built-in set."));
                empty.add_css_class("dim-label");
                empty.set_margin_top(12);
                empty.set_margin_bottom(12);
                list.append(&empty);
            }
            for (idx, action) in actions.iter().enumerate() {
                let rerender = render_cell_for_closure.borrow().clone();
                list.append(&build_action_row(action, idx, state.clone(), rerender));
            }
        });
        *render_cell.borrow_mut() = Some(render.clone());
        render
    };
    render();

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let add_btn = gtk::Button::with_label("Add Action");
    add_btn.add_css_class("suggested-action");
    {
        let state = state.clone();
        let render = render.clone();
        add_btn.connect_clicked(move |_| {
            {
                let mut s = state.borrow_mut();
                s.custom_actions.push(CustomAction {
                    name: "New Action".to_string(),
                    command: String::new(),
                    args: vec!["{path}".to_string()],
                    icon: None,
                    show_for: ShowFor::All,
                    multiple: false,
                });
                persist(&s.custom_actions);
            }
            render();
        });
    }
    buttons.append(&add_btn);

    let restore_btn = gtk::Button::with_label("Restore Built-in Actions");
    {
        let state = state.clone();
        let render = render.clone();
        restore_btn.connect_clicked(move |_| {
            {
                let mut s = state.borrow_mut();
                s.custom_actions = custom_actions::builtin();
                persist(&s.custom_actions);
            }
            render();
        });
    }
    buttons.append(&restore_btn);

    content.append(&group);
    content.append(&buttons);
    scroll.set_child(Some(&content));
    scroll
}

fn persist(actions: &[CustomAction]) {
    if let Err(e) = custom_actions::save(actions) {
        tracing::warn!("Failed to save custom actions: {}", e);
    }
}

fn build_action_row(
    action: &CustomAction,
    idx: usize,
    state: AppState,
    rerender: Option<Rc<dyn Fn()>>,
) -> gtk::Frame {
    let frame = gtk::Frame::new(None);
    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(12);
    grid.set_margin_start(12);
    grid.set_margin_end(12);
    grid.set_margin_top(8);
    grid.set_margin_bottom(8);

    // Every field writes straight into the action at `idx`; the list is
    // re-rendered after a delete so indexes stay right.
    let update = {
        let state = state.clone();
        Rc::new(move |f: &dyn Fn(&mut CustomAction)| {
            let mut s = state.borrow_mut();
            if let Some(a) = s.custom_actions.get_mut(idx) {
                f(a);
                persist(&s.custom_actions);
            }
        })
    };

    let mut row = 0;
    let mut add_labeled = |label: &str, widget: &gtk::Widget| {
        let lbl = gtk::Label::new(Some(label));
        lbl.set_halign(gtk::Align::End);
        lbl.add_css_class("dim-label");
        grid.attach(&lbl, 0, row, 1, 1);
        widget.set_hexpand(true);
        grid.attach(widget, 1, row, 1, 1);
        row += 1;
    };

    let name_entry = gtk::Entry::new();
    name_entry.set_text(&action.name);
    name_entry.set_placeholder_text(Some("Menu label"));
    {
        let update = update.clone();
        name_entry.connect_changed(move |e| {
            let text = e.text().to_string();
            update(&|a| a.name = text.clone());
        });
    }
    add_labeled("Name", name_entry.upcast_ref());

    let command_entry = gtk::Entry::new();
    command_entry.set_text(&action.command);
    command_entry.set_placeholder_text(Some("Program to run, e.g. file-roller"));
    {
        let update = update.clone();
        command_entry.connect_changed(move |e| {
            let text = e.text().trim().to_string();
            update(&|a| a.command = text.clone());
        });
    }
    add_labeled("Command", command_entry.upcast_ref());

    let args_entry = gtk::Entry::new();
    args_entry.set_text(&join_args(&action.args));
    args_entry.set_placeholder_text(Some("Arguments, e.g. --add {paths}"));
    {
        let update = update.clone();
        args_entry.connect_changed(move |e| {
            let args = split_args(&e.text());
            update(&|a| a.args = args.clone());
        });
    }
    add_labeled("Arguments", args_entry.upcast_ref());

    let icon_entry = gtk::Entry::new();
    icon_entry.set_text(action.icon.as_deref().unwrap_or(""));
    icon_entry.set_placeholder_text(Some("Icon name (optional), e.g. utilities-terminal-symbolic"));
    {
        let update = update.clone();
        icon_entry.connect_changed(move |e| {
            let text = e.text().trim().to_string();
            update(&|a| a.icon = (!text.is_empty()).then(|| text.clone()));
        });
    }
    add_labeled("Icon", icon_entry.upcast_ref());

    let options = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let labels: Vec<&str> = ShowFor::ALL.iter().map(|s| s.label()).collect();
    let show_for = gtk::DropDown::from_strings(&labels);
    show_for.set_selected(
        ShowFor::ALL
            .iter()
            .position(|s| *s == action.show_for)
            .unwrap_or(0) as u32,
    );
    {
        let update = update.clone();
        show_for.connect_selected_notify(move |d| {
            let chosen = ShowFor::ALL
                .get(d.selected() as usize)
                .copied()
                .unwrap_or_default();
            update(&|a| a.show_for = chosen);
        });
    }
    options.append(&show_for);

    let multiple = gtk::CheckButton::with_label("Also for multiple selected items");
    multiple.set_active(action.multiple);
    {
        let update = update.clone();
        multiple.connect_toggled(move |c| {
            let on = c.is_active();
            update(&|a| a.multiple = on);
        });
    }
    options.append(&multiple);

    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    options.append(&spacer);

    let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("destructive-action");
    delete_btn.set_tooltip_text(Some("Remove action"));
    {
        let state = state.clone();
        delete_btn.connect_clicked(move |_| {
            {
                let mut s = state.borrow_mut();
                if idx < s.custom_actions.len() {
                    s.custom_actions.remove(idx);
                    persist(&s.custom_actions);
                }
            }
            if let Some(rerender) = &rerender {
                rerender();
            }
        });
    }
    options.append(&delete_btn);
    add_labeled("Show for", options.upcast_ref());

    frame.set_child(Some(&grid));
    frame
}
