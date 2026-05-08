use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::prelude::*;
use adw::prelude::*;
use gtk::{gio, glib};

use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};

pub fn build_ui(app: &adw::Application, engine: Rc<RefCell<TorrentEngine>>) {
    // ── Channel for engine → UI updates ──
    let (tx, rx) = async_channel::unbounded();
    engine.borrow_mut().set_sender(tx);

    // ── Header bar ──
    let header = adw::HeaderBar::new();

    let title = adw::WindowTitle::new("Rill", "");
    header.set_title_widget(Some(&title));

    let add_btn = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Torrent")
        .build();
    header.pack_end(&add_btn);

    let menu_model = gio::Menu::new();
    menu_model.append(Some("About Rill"), Some("app.about"));
    menu_model.append(Some("Quit"), Some("app.quit"));
    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .build();
    header.pack_end(&menu_btn);

    // ── App actions ──
    let app_clone = app.clone();
    let about_action = gio::SimpleAction::new("about", None);
    about_action.connect_activate(move |_, _| {
        log::info!("Rill v0.1.0 — Minimalistic BitTorrent client for GNOME");
    });
    app.add_action(&about_action);

    let app_clone2 = app.clone();
    let quit_action = gio::SimpleAction::new("quit", None);
    quit_action.connect_activate(move |_, _| {
        app_clone2.quit();
    });
    app.add_action(&quit_action);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    // ── Torrent list ──
    let download_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["rich-list", "boxed-list"])
        .build();

    let dl_header_label = gtk::Label::builder()
        .label("Downloading")
        .halign(gtk::Align::Start)
        .margin_start(12)
        .margin_top(12)
        .css_classes(["heading"])
        .build();

    let pause_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["rich-list", "boxed-list"])
        .visible(false)
        .build();

    let pause_header_label = gtk::Label::builder()
        .label("Paused")
        .halign(gtk::Align::Start)
        .margin_start(12)
        .margin_top(12)
        .css_classes(["heading"])
        .visible(false)
        .build();

    let done_list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .css_classes(["rich-list", "boxed-list"])
        .visible(false)
        .build();

    let done_header_label = gtk::Label::builder()
        .label("Completed")
        .halign(gtk::Align::Start)
        .margin_start(12)
        .margin_top(12)
        .css_classes(["heading"])
        .visible(false)
        .build();

    // ── Empty state ──
    let empty_label = gtk::Label::builder()
        .label("No Torrents\n\nAdd a torrent to start downloading")
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .hexpand(true)
        .build();
    empty_label.add_css_class("dim-label");

    let stack = gtk::Stack::new();
    stack.add_named(&empty_label, Some("empty"));
    stack.set_visible_child_name("empty");

    let content_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content_box.append(&stack);

    let dl_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    dl_section.append(&dl_header_label);
    dl_section.append(&download_list);
    content_box.append(&dl_section);

    let pause_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    pause_section.append(&pause_header_label);
    pause_section.append(&pause_list);
    content_box.append(&pause_section);

    let done_section = gtk::Box::new(gtk::Orientation::Vertical, 0);
    done_section.append(&done_header_label);
    done_section.append(&done_list);
    content_box.append(&done_section);

    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&content_box));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroll));

    // ── Window ──
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .content(&toolbar)
        .default_width(480)
        .default_height(640)
        .title("Rill")
        .build();

    // ── Torrent rows tracker ──
    let torrent_rows: Rc<RefCell<HashMap<String, RillRowWidgets>>> =
        Rc::new(RefCell::new(HashMap::new()));

    // ── Add torrent dialog ──
    let engine_add = Rc::downgrade(&engine);
    add_btn.connect_clicked(glib::clone!(
        #[weak] window,
        move |_| {
            if let Some(engine) = engine_add.upgrade() {
                show_add_dialog(&window, engine.clone());
            }
        }
    ));

    // ── Process engine updates ──
    glib::spawn_future_local(glib::clone!(
        #[strong] torrent_rows,
        #[strong(rename_to = dl_list)] download_list,
        #[strong(rename_to = p_list)] pause_list,
        #[strong(rename_to = d_list)] done_list,
        #[strong(rename_to = dl_header)] dl_header_label,
        #[strong(rename_to = p_header)] pause_header_label,
        #[strong(rename_to = d_header)] done_header_label,
        #[strong] stack,
        async move {
            while let Ok(event) = rx.recv().await {
                match event {
                    UiEvent::Update(update) => {
                        let mut rows = torrent_rows.borrow_mut();
                        let row = if let Some(r) = rows.get(&update.info_hash) {
                            r.clone()
                        } else {
                            let r = make_torrent_row(&update);
                            rows.insert(update.info_hash.clone(), r.clone());
                            r
                        };
                        update_row(&row, &update);

                        // Move row to correct list
                        let (target_list, target_header) = match update.state {
                            TorrentUiState::Downloading => (&dl_list, &dl_header),
                            TorrentUiState::Paused | TorrentUiState::Error => (&p_list, &p_header),
                            TorrentUiState::Completed => (&d_list, &d_header),
                        };

                        if row.container.parent().as_ref() != Some(target_list.upcast_ref()) {
                            if let Some(p) = row.container.parent() {
                                if let Ok(lb) = p.downcast::<gtk::ListBox>() {
                                    lb.remove(&row.container);
                                }
                            }
                            target_list.append(&row.container);
                            target_header.set_visible(true);
                            target_list.set_visible(true);
                        }

                        stack.set_visible_child_name("torrents");
                    }
                    UiEvent::Finished { info_hash, error } => {
                        if let Some(err) = error {
                            log::error!("{info_hash}: {err}");
                        }
                    }
                }
            }
        }
    ));

    window.present();
}

// ── Torrent row widget bundle ──
#[derive(Clone)]
struct RillRowWidgets {
    container: gtk::ListBoxRow,
    name_label: gtk::Label,
    status_label: gtk::Label,
    progress: gtk::ProgressBar,
    index_stack: gtk::Stack,
    action_stack: gtk::Stack,
    is_active: Rc<RefCell<bool>>,
}

fn make_torrent_row(update: &UiUpdate) -> RillRowWidgets {
    // Left icon
    let index_stack = gtk::Stack::new();
    index_stack.set_width_request(56);
    index_stack.set_halign(gtk::Align::Center);
    index_stack.set_valign(gtk::Align::Center);

    let dl_icon = gtk::Image::builder()
        .icon_name("document-save-symbolic")
        .pixel_size(16)
        .css_classes(["bubble"])
        .build();
    let pause_icon = gtk::Image::builder()
        .icon_name("media-playback-pause-symbolic")
        .pixel_size(16)
        .css_classes(["bubble"])
        .build();
    let done_icon = gtk::Image::builder()
        .icon_name("emblem-ok-symbolic")
        .pixel_size(16)
        .css_classes(["bubble"])
        .build();
    let err_icon = gtk::Image::builder()
        .icon_name("dialog-error-symbolic")
        .pixel_size(16)
        .css_classes(["bubble"])
        .build();

    index_stack.add_named(&dl_icon, Some("downloading"));
    index_stack.add_named(&pause_icon, Some("paused"));
    index_stack.add_named(&done_icon, Some("completed"));
    index_stack.add_named(&err_icon, Some("error"));

    // Info
    let name_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .xalign(0.0)
        .css_classes(["heading"])
        .build();

    let status_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .single_line_mode(true)
        .xalign(0.0)
        .css_classes(["caption"])
        .build();

    let progress = gtk::ProgressBar::builder()
        .show_text(false)
        .build();

    let info_box = gtk::Box::new(gtk::Orientation::Vertical, 3);
    info_box.set_hexpand(true);
    info_box.set_margin_top(10);
    info_box.set_margin_bottom(10);
    info_box.set_margin_end(12);
    info_box.set_valign(gtk::Align::Center);
    info_box.append(&name_label);
    info_box.append(&status_label);
    info_box.append(&progress);

    // Action button
    let action_stack = gtk::Stack::new();
    action_stack.set_halign(gtk::Align::Center);
    action_stack.set_valign(gtk::Align::Center);
    action_stack.set_margin_end(12);

    let pause_btn = gtk::Button::builder()
        .icon_name("media-playback-pause-symbolic")
        .tooltip_text("Pause")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let resume_btn = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text("Resume")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let remove_btn = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Remove")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();

    action_stack.add_named(&pause_btn, Some("pause"));
    action_stack.add_named(&resume_btn, Some("resume"));
    action_stack.add_named(&remove_btn, Some("remove"));

    // Row layout
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    hbox.append(&index_stack);
    hbox.append(&info_box);
    hbox.append(&action_stack);

    let container = gtk::ListBoxRow::new();
    container.set_child(Some(&hbox));

    let is_active = Rc::new(RefCell::new(true));

    // Right-click context menu
    let gesture = gtk::GestureClick::new();
    gesture.set_button(3);
    let is_active_rc = is_active.clone();
    let action_stack_rc = action_stack.clone();
    let index_stack_rc = index_stack.clone();
    let container_rc = container.clone();
    gesture.connect_pressed(move |gesture, _n, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        show_context_menu(
            &container_rc,
            &action_stack_rc,
            &index_stack_rc,
            &is_active_rc,
            x,
            y,
        );
    });
    container.add_controller(gesture);

    RillRowWidgets {
        container,
        name_label,
        status_label,
        progress,
        index_stack,
        action_stack,
        is_active,
    }
}

fn update_row(row: &RillRowWidgets, update: &UiUpdate) {
    let name = if update.name.is_empty() {
        torrent_name(&update.uri)
    } else {
        update.name.clone()
    };
    row.name_label.set_label(&name);

    if update.total > 0 {
        row.progress
            .set_fraction(update.downloaded as f64 / update.total as f64);
    } else {
        row.progress.pulse();
    }

    let status = format!("{} / {} · {} peers · {}/s",
        fmt_bytes(update.downloaded),
        fmt_bytes(update.total),
        update.peers,
        fmt_speed(update.speed_down),
    );
    row.status_label.set_label(&status);

    match update.state {
        TorrentUiState::Downloading => {
            *row.is_active.borrow_mut() = true;
            row.index_stack.set_visible_child_name("downloading");
            row.action_stack.set_visible_child_name("pause");
        }
        TorrentUiState::Paused => {
            *row.is_active.borrow_mut() = false;
            row.index_stack.set_visible_child_name("paused");
            row.action_stack.set_visible_child_name("resume");
        }
        TorrentUiState::Completed => {
            *row.is_active.borrow_mut() = false;
            row.index_stack.set_visible_child_name("completed");
            row.action_stack.set_visible_child_name("remove");
            row.progress.set_fraction(1.0);
        }
        TorrentUiState::Error => {
            *row.is_active.borrow_mut() = false;
            row.index_stack.set_visible_child_name("error");
            row.action_stack.set_visible_child_name("remove");
        }
    }
}

fn show_context_menu(
    parent: &gtk::ListBoxRow,
    action_stack: &gtk::Stack,
    index_stack: &gtk::Stack,
    is_active: &Rc<RefCell<bool>>,
    x: f64,
    y: f64,
) {
    let menu = gio::Menu::new();

    if *is_active.borrow() {
        menu.append(Some("_Pause"), Some("ctx.pause"));
    } else {
        menu.append(Some("_Resume"), Some("ctx.resume"));
    }
    menu.append(Some("_Remove"), Some("ctx.remove"));

    let actions = gio::SimpleActionGroup::new();

    let pause_action = gio::SimpleAction::new("pause", None);
    pause_action.connect_activate({
        let is_a = is_active.clone();
        let as_s = action_stack.clone();
        let is_s = index_stack.clone();
        move |_, _| {
            *is_a.borrow_mut() = false;
            as_s.set_visible_child_name("resume");
            is_s.set_visible_child_name("paused");
        }
    });
    actions.add_action(&pause_action);

    let resume_action = gio::SimpleAction::new("resume", None);
    resume_action.connect_activate({
        let is_a = is_active.clone();
        let as_s = action_stack.clone();
        let is_s = index_stack.clone();
        move |_, _| {
            *is_a.borrow_mut() = true;
            as_s.set_visible_child_name("pause");
            is_s.set_visible_child_name("downloading");
        }
    });
    actions.add_action(&resume_action);

    let parent_rm = parent.clone();
    let remove_action = gio::SimpleAction::new("remove", None);
    remove_action.connect_activate(move |_, _| {
        if let Some(p) = parent_rm.parent() {
            if let Ok(lb) = p.downcast::<gtk::ListBox>() {
                lb.remove(&parent_rm);
            }
        }
    });
    actions.add_action(&remove_action);

    parent.insert_action_group("ctx", Some(&actions));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(parent.upcast_ref::<gtk::Widget>());
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.popup();
}

// ── Add dialog ──
fn show_add_dialog(parent: &adw::ApplicationWindow, engine: Rc<RefCell<TorrentEngine>>) {
    let dlg = gtk::Window::builder()
        .title("Add Torrent")
        .default_width(420)
        .default_height(280)
        .transient_for(parent)
        .modal(true)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);

    let entry = adw::EntryRow::new();
    entry.set_title("Magnet Link or File Path");
    content.append(&entry);

    let downloads = dirs_next::download_dir().unwrap_or_else(|| PathBuf::from("."));
    let output_dir = Rc::new(RefCell::new(downloads.clone()));

    let out_row = adw::ActionRow::new();
    out_row.set_title("Output Folder");
    out_row.set_subtitle(&downloads.display().to_string());

    let pick_btn = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .tooltip_text("Browse")
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .build();
    out_row.add_suffix(&pick_btn);
    out_row.set_activatable_widget(Some(&pick_btn));
    content.append(&out_row);

    let bbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bbox.set_halign(gtk::Align::End);
    bbox.set_margin_top(12);

    let cancel_btn = gtk::Button::builder().label("_Cancel").use_underline(true).build();
    let add_btn = gtk::Button::builder()
        .label("_Add")
        .use_underline(true)
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    bbox.append(&cancel_btn);
    bbox.append(&add_btn);
    content.append(&bbox);

    dlg.set_child(Some(&content));

    entry.connect_text_notify(glib::clone!(
        #[weak] add_btn,
        move |e| {
            add_btn.set_sensitive(!e.text().trim().is_empty());
        }
    ));

    cancel_btn.connect_clicked(glib::clone!(
        #[weak] dlg,
        move |_| dlg.close()
    ));

    add_btn.connect_clicked(glib::clone!(
        #[weak] dlg,
        #[weak] entry,
        #[strong] output_dir,
        #[strong] engine,
        move |_| {
            let uri = entry.text().to_string();
            let dir = output_dir.borrow().clone();
            engine.borrow_mut().start(uri, dir);
            dlg.close();
        }
    ));

    let output_dir_pick = output_dir.clone();
    let out_row_pick = out_row.clone();
    pick_btn.connect_clicked(glib::clone!(
        #[weak] dlg,
        #[strong] output_dir_pick,
        #[strong] out_row_pick,
        move |_| {
            let fd = gtk::FileDialog::builder()
                .title("Select Output Folder")
                .accept_label("_Select")
                .build();
            fd.select_folder(Some(&dlg), gio::Cancellable::NONE, glib::clone!(
                #[strong] output_dir_pick,
                #[strong] out_row_pick,
                move |result| {
                    if let Ok(folder) = result {
                        if let Some(path) = folder.path() {
                            out_row_pick.set_subtitle(&path.display().to_string());
                            *output_dir_pick.borrow_mut() = path;
                        }
                    }
                }
            ));
        }
    ));

    dlg.present();
}

// ── Helpers ──
fn torrent_name(uri: &str) -> String {
    if let Some(pos) = uri.rfind('/') {
        uri[pos + 1..].to_string()
    } else if let Some(start) = uri.find("dn=") {
        let after = &uri[start + 3..];
        after.find('&').map(|e| &after[..e]).unwrap_or(after).to_string()
    } else {
        uri.to_string()
    }
}

fn fmt_bytes(bytes: u64) -> String {
    const U: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut i = 0;
    while size >= 1024. && i < U.len() - 1 { size /= 1024.; i += 1; }
    if i == 0 { format!("{} B", bytes) } else { format!("{:.1} {}", size, U[i]) }
}

fn fmt_speed(bps: u64) -> String {
    if bps == 0 { return "0 B".into(); }
    if bps >= 1048576 { return format!("{:.1} MB/s", bps as f64 / 1048576.); }
    if bps >= 1024 { return format!("{:.0} KB/s", bps as f64 / 1024.); }
    format!("{} B/s", bps)
}
