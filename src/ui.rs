use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use glib::clone;

use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};

const APP_CSS: &str = "
progressbar.thin > trough,
progressbar.thin > trough > progress {
    min-height: 4px;
}
.torrent-icon.accent {
    color: @accent_color;
}
.torrent-icon.success {
    color: @success_color;
}
.torrent-icon.error {
    color: @error_color;
}
.torrent-icon.dim {
    opacity: 0.55;
}
";

pub fn build_ui(app: &adw::Application, engine: Rc<RefCell<TorrentEngine>>) {
    let css = gtk::CssProvider::new();
    css.load_from_string(APP_CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let (tx, rx) = async_channel::unbounded();
    engine.borrow_mut().set_sender(tx);

    // ── Header bar ──
    let title = adw::WindowTitle::new("Rill", "");

    let add_btn = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add Torrent")
        .build();

    let main_menu = gio::Menu::new();
    main_menu.append(Some("_About Rill"), Some("app.about"));
    main_menu.append(Some("_Quit"), Some("app.quit"));
    let menu_btn = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&main_menu)
        .primary(true)
        .build();

    let header = adw::HeaderBar::builder()
        .title_widget(&title)
        .build();
    header.pack_start(&add_btn);
    header.pack_end(&menu_btn);

    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(|_, _| { log::info!("Rill v0.1.0"); });
    app.add_action(&about);

    let app_q = app.clone();
    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(move |_, _| app_q.quit());
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    // ── Empty state ──
    let empty = adw::StatusPage::builder()
        .icon_name("folder-download-symbolic")
        .title("No Torrents")
        .description("Add a magnet link or .torrent file to get started")
        .vexpand(true)
        .hexpand(true)
        .build();

    // ── Content sections ──
    let dl_header = section_label("Downloading");
    dl_header.set_visible(false);
    let dl_list = make_listbox();
    dl_list.set_visible(false);

    let pause_header = section_label("Paused");
    pause_header.set_visible(false);
    let pause_list = make_listbox();
    pause_list.set_visible(false);

    let done_header = section_label("Completed");
    done_header.set_visible(false);
    let done_list = make_listbox();
    done_list.set_visible(false);

    let content_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    content_box.append(&empty);
    content_box.append(&dl_header);
    content_box.append(&dl_list);
    content_box.append(&pause_header);
    content_box.append(&pause_list);
    content_box.append(&done_header);
    content_box.append(&done_list);

    let clamp = adw::Clamp::builder()
        .maximum_size(760)
        .margin_top(24)
        .margin_bottom(24)
        .margin_start(16)
        .margin_end(16)
        .child(&content_box)
        .build();

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&scroll));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Rill")
        .default_width(560)
        .default_height(680)
        .content(&toolbar_view)
        .build();

    // ── Shared row map ──
    let rows: Rc<RefCell<HashMap<String, RillRow>>> = Rc::new(RefCell::new(HashMap::new()));

    // ── Add button ──
    add_btn.connect_clicked(clone!(
        #[weak] window,
        #[strong] engine,
        move |_| show_add_dialog(&window, engine.clone())
    ));

    // ── Preferences action ──
    let prefs_win = window.clone();
    let prefs_action = gio::SimpleAction::new("preferences", None);
    prefs_action.connect_activate(move |_, _| show_preferences(&prefs_win));
    app.add_action(&prefs_action);
    app.set_accels_for_action("app.preferences", &["<Control>comma"]);

    // ── Process engine updates ──
    let rx_cell: Rc<RefCell<async_channel::Receiver<UiEvent>>> = Rc::new(RefCell::new(rx));
    glib::timeout_add_local(Duration::from_millis(200), clone!(
        #[strong] rows,
        #[strong(rename_to = dl)] dl_list,
        #[strong(rename_to = pl)] pause_list,
        #[strong(rename_to = cl)] done_list,
        #[strong(rename_to = dlh)] dl_header,
        #[strong(rename_to = ph)] pause_header,
        #[strong(rename_to = ch)] done_header,
        #[strong] empty,
        #[strong] engine,
        #[strong] rx_cell,
        move || {
            let rx = rx_cell.borrow();
            while let Ok(event) = rx.try_recv() {
                match event {
                    UiEvent::Update(update) => {
                        let mut map = rows.borrow_mut();
                        let row = if let Some(r) = map.get(&update.info_hash) {
                            r.clone()
                        } else {
                            let r = RillRow::new(&update, &engine);
                            map.insert(update.info_hash.clone(), r.clone());
                            r
                        };
                        row.state.set(update.state.clone());
                        row.update(&update);

                        let (target, target_header) = match update.state {
                            TorrentUiState::Downloading => (&dl, &dlh),
                            TorrentUiState::Paused => (&pl, &ph),
                            TorrentUiState::Completed | TorrentUiState::Error => (&cl, &ch),
                        };

                        let cur = row.container.parent()
                            .and_then(|p| p.downcast::<gtk::ListBox>().ok());
                        if cur.as_ref() != Some(target) {
                            if let Some(lb) = cur { lb.remove(&row.container); }
                            target.append(&row.container);
                        }

                        target.set_visible(true);
                        target_header.set_visible(true);
                        empty.set_visible(false);
                    }
                    UiEvent::Finished { info_hash, error } => {
                        if let Some(e) = error {
                            log::error!("Torrent {info_hash}: {e}");
                        }
                        let map = rows.borrow();
                        if map.is_empty() {
                            empty.set_visible(true);
                            dl_header.set_visible(false);
                            dl.set_visible(false);
                            pause_header.set_visible(false);
                            pl.set_visible(false);
                            done_header.set_visible(false);
                            cl.set_visible(false);
                        }
                    }
                }
            }
            glib::ControlFlow::Continue
        }
    ));

    // ── Preferences in menu ──
    main_menu.insert(0, Some("_Preferences"), Some("app.preferences"));

    window.present();
}

fn section_label(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["title-4"])
        .margin_start(6)
        .margin_top(12)
        .build()
}

fn make_listbox() -> gtk::ListBox {
    gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build()
}

// ── Torrent row ──
#[derive(Clone)]
struct RillRow {
    container: gtk::ListBoxRow,
    name_lbl: gtk::Label,
    status_lbl: gtk::Label,
    progress: gtk::ProgressBar,
    icon: gtk::Image,
    action_btn: gtk::Button,
    state: Rc<Cell<TorrentUiState>>,
}

impl RillRow {
    fn new(update: &UiUpdate, engine: &Rc<RefCell<TorrentEngine>>) -> Self {
        let icon = gtk::Image::builder()
            .icon_name("folder-download-symbolic")
            .pixel_size(24)
            .css_classes(["torrent-icon"])
            .margin_top(4)
            .margin_bottom(4)
            .build();

        let icon_wrap = gtk::Box::builder()
            .valign(gtk::Align::Start)
            .halign(gtk::Align::Center)
            .width_request(32)
            .build();
        icon_wrap.append(&icon);

        let name_lbl = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .xalign(0.0)
            .css_classes(["heading"])
            .build();

        let status_lbl = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build();

        let progress = gtk::ProgressBar::builder()
            .hexpand(true)
            .css_classes(["thin"])
            .build();

        let info_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        info_box.append(&name_lbl);
        info_box.append(&status_lbl);
        info_box.append(&progress);

        let action_btn = gtk::Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();

        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        row_box.append(&icon_wrap);
        row_box.append(&info_box);
        row_box.append(&action_btn);

        let container = gtk::ListBoxRow::builder()
            .activatable(false)
            .selectable(false)
            .child(&row_box)
            .build();

        let initial_state = update.state.clone();
        let shared_state: Rc<Cell<TorrentUiState>> = Rc::new(Cell::new(initial_state));

        // ── Right-click context menu ──
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        let c_eng = engine.clone();
        let c_icon = icon.clone();
        let c_btn = action_btn.clone();
        let c_state = Rc::clone(&shared_state);
        let c_hash = update.info_hash.clone();
        let c_uri = update.uri.clone();
        let c_out = update.output_dir.clone();
        let c_container = container.clone();
        gesture.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            show_context_menu(
                &c_container, &c_btn, &c_icon,
                c_state.get(), c_eng.clone(),
                &c_hash, &c_uri, &c_out,
                x, y,
            );
        });
        container.add_controller(gesture);

        // ── Action button click ──
        let b_eng = engine.clone();
        let b_icon = icon.clone();
        let b_btn = action_btn.clone();
        let b_state = Rc::clone(&shared_state);
        let b_hash = update.info_hash.clone();
        let b_uri = update.uri.clone();
        let b_out = update.output_dir.clone();
        let b_container = container.clone();
        action_btn.connect_clicked(move |_| {
            match b_state.get() {
                TorrentUiState::Downloading => {
                    b_eng.borrow().toggle(&b_hash);
                    b_state.set(TorrentUiState::Paused);
                    apply_row_visual(&b_btn, &b_icon, TorrentUiState::Paused);
                }
                TorrentUiState::Paused => {
                    b_eng.borrow().start(b_uri.clone(), b_out.clone());
                    b_state.set(TorrentUiState::Downloading);
                    apply_row_visual(&b_btn, &b_icon, TorrentUiState::Downloading);
                }
                TorrentUiState::Completed | TorrentUiState::Error => {
                    if let Some(p) = b_container.parent() {
                        if let Ok(lb) = p.downcast::<gtk::ListBox>() {
                            lb.remove(&b_container);
                        }
                    }
                }
            }
        });

        let row = Self {
            container,
            name_lbl,
            status_lbl,
            progress,
            icon,
            action_btn,
            state: Rc::clone(&shared_state),
        };
        row.update(update);
        row
    }

    fn update(&self, u: &UiUpdate) {
        let name = if u.name.is_empty() { fallback_name(&u.uri) } else { u.name.clone() };
        self.name_lbl.set_label(&name);

        self.progress.set_show_text(false);
        if u.total > 0 {
            self.progress.set_fraction(u.downloaded as f64 / u.total as f64);
        } else if u.state == TorrentUiState::Downloading {
            self.progress.pulse();
        }

        let status = match u.state {
            TorrentUiState::Completed => format!("{}", fmt_bytes(u.total)),
            TorrentUiState::Paused => format!("{} of {} — Paused", fmt_bytes(u.downloaded), fmt_bytes(u.total)),
            TorrentUiState::Error => "Error".to_string(),
            TorrentUiState::Downloading => format!(
                "{} of {} · {} peers · {}/s",
                fmt_bytes(u.downloaded),
                fmt_bytes(u.total),
                u.peers,
                fmt_speed(u.speed_down),
            ),
        };
        self.status_lbl.set_label(&status);

        apply_row_visual(&self.action_btn, &self.icon, u.state);
    }
}

fn apply_row_visual(btn: &gtk::Button, icon: &gtk::Image, state: TorrentUiState) {
    match state {
        TorrentUiState::Downloading => {
            btn.set_icon_name("media-playback-pause-symbolic");
            icon.set_icon_name(Some("folder-download-symbolic"));
            icon.set_css_classes(&["torrent-icon", "accent"]);
        }
        TorrentUiState::Paused => {
            btn.set_icon_name("media-playback-start-symbolic");
            icon.set_icon_name(Some("media-playback-pause-symbolic"));
            icon.set_css_classes(&["torrent-icon", "dim"]);
        }
        TorrentUiState::Completed => {
            btn.set_icon_name("user-trash-symbolic");
            icon.set_icon_name(Some("emblem-ok-symbolic"));
            icon.set_css_classes(&["torrent-icon", "success"]);
        }
        TorrentUiState::Error => {
            btn.set_icon_name("user-trash-symbolic");
            icon.set_icon_name(Some("dialog-error-symbolic"));
            icon.set_css_classes(&["torrent-icon", "error"]);
        }
    }
}

// ── Context menu ──
fn show_context_menu(
    parent: &gtk::ListBoxRow,
    action_btn: &gtk::Button,
    icon: &gtk::Image,
    state: TorrentUiState,
    engine: Rc<RefCell<TorrentEngine>>,
    info_hash: &str,
    uri: &str,
    output_dir: &PathBuf,
    x: f64,
    y: f64,
) {
    let menu = gio::Menu::new();

    match state {
        TorrentUiState::Downloading => {
            menu.append(Some("_Pause"), Some("ctx.pause"));
            menu.append(Some("_Remove"), Some("ctx.remove"));
        }
        TorrentUiState::Paused => {
            menu.append(Some("_Resume"), Some("ctx.resume"));
            menu.append(Some("_Remove"), Some("ctx.remove"));
        }
        TorrentUiState::Completed | TorrentUiState::Error => {
            menu.append(Some("_Remove"), Some("ctx.remove"));
        }
    }

    let group = gio::SimpleActionGroup::new();

    // Pause
    {
        let h = info_hash.to_string();
        let eng = engine.clone();
        let a = action_btn.clone();
        let i = icon.clone();
        let pause = gio::SimpleAction::new("pause", None);
        pause.connect_activate(move |_, _| {
            eng.borrow().toggle(&h);
            apply_row_visual(&a, &i, TorrentUiState::Paused);
        });
        group.add_action(&pause);
    }

    // Resume
    {
        let u = uri.to_string();
        let o = output_dir.clone();
        let eng = engine.clone();
        let a = action_btn.clone();
        let i = icon.clone();
        let resume = gio::SimpleAction::new("resume", None);
        resume.connect_activate(move |_, _| {
            eng.borrow().start(u.clone(), o.clone());
            apply_row_visual(&a, &i, TorrentUiState::Downloading);
        });
        group.add_action(&resume);
    }

    // Remove
    {
        let h = info_hash.to_string();
        let p = parent.clone();
        let eng = engine.clone();
        let remove = gio::SimpleAction::new("remove", None);
        remove.connect_activate(move |_, _| {
            eng.borrow().toggle(&h);
            if let Some(pw) = p.parent() {
                if let Ok(lb) = pw.downcast::<gtk::ListBox>() { lb.remove(&p); }
            }
        });
        group.add_action(&remove);
    }

    parent.insert_action_group("ctx", Some(&group));

    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_parent(parent.upcast_ref::<gtk::Widget>());
    popover.set_has_arrow(false);
    popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
    popover.popup();
}

// ── Add dialog ──
fn show_add_dialog(parent: &adw::ApplicationWindow, engine: Rc<RefCell<TorrentEngine>>) {
    let dlg = adw::Window::builder()
        .title("Add Torrent")
        .default_width(440)
        .default_height(280)
        .transient_for(parent)
        .modal(true)
        .build();

    let header = adw::HeaderBar::builder()
        .show_end_title_buttons(false)
        .show_start_title_buttons(false)
        .build();

    let cancel_btn = gtk::Button::builder().label("Cancel").build();
    let add_btn = gtk::Button::builder()
        .label("Add")
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    header.pack_start(&cancel_btn);
    header.pack_end(&add_btn);

    let downloads = dirs_next::download_dir().unwrap_or_else(|| PathBuf::from("."));
    let out_dir = Rc::new(RefCell::new(downloads.clone()));

    let group = adw::PreferencesGroup::new();
    let entry = adw::EntryRow::builder()
        .title("Magnet Link or .torrent Path")
        .build();
    group.add(&entry);

    let out_row = adw::ActionRow::builder()
        .title("Save to")
        .subtitle(&downloads.display().to_string())
        .activatable(true)
        .build();
    let pick_icon = gtk::Image::from_icon_name("folder-symbolic");
    out_row.add_suffix(&pick_icon);
    group.add(&out_row);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(24)
        .margin_start(24)
        .margin_end(24)
        .build();
    body.append(&group);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    dlg.set_content(Some(&toolbar));

    entry.connect_changed(clone!(#[weak] add_btn, move |e| {
        add_btn.set_sensitive(!e.text().trim().is_empty());
    }));

    cancel_btn.connect_clicked(clone!(#[weak] dlg, move |_| dlg.close()));

    add_btn.connect_clicked(clone!(
        #[weak] dlg,
        #[weak] entry,
        #[strong] out_dir,
        #[strong] engine,
        move |_| {
            let uri = entry.text().to_string();
            engine.borrow().start(uri, out_dir.borrow().clone());
            dlg.close();
        }
    ));

    out_row.connect_activated(clone!(
        #[weak] dlg,
        #[strong] out_dir,
        #[strong] out_row,
        move |_| {
            let fd = gtk::FileDialog::builder()
                .title("Select Folder")
                .accept_label("Select")
                .build();
            fd.select_folder(Some(&dlg), gio::Cancellable::NONE, clone!(
                #[strong] out_dir,
                #[strong] out_row,
                move |r| {
                    if let Ok(f) = r {
                        if let Some(p) = f.path() {
                            out_row.set_subtitle(&p.display().to_string());
                            *out_dir.borrow_mut() = p;
                        }
                    }
                }
            ));
        }
    ));

    dlg.present();
}

// ── Preferences ──
fn show_preferences(parent: &adw::ApplicationWindow) {
    let win = adw::PreferencesWindow::builder()
        .title("Rill Preferences")
        .transient_for(parent)
        .modal(true)
        .build();

    let general = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let dl_group = adw::PreferencesGroup::builder()
        .title("Downloads")
        .build();

    let downloads = dirs_next::download_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir_row = adw::ActionRow::builder()
        .title("Download Folder")
        .subtitle(&downloads.display().to_string())
        .activatable(true)
        .build();
    let pick_icon = gtk::Image::from_icon_name("folder-symbolic");
    dir_row.add_suffix(&pick_icon);
    dl_group.add(&dir_row);

    let dir_row_c = dir_row.clone();
    let win_c = win.clone();
    dir_row.connect_activated(move |_| {
        let fd = gtk::FileDialog::builder()
            .title("Select Download Folder")
            .accept_label("Select")
            .build();
        let row = dir_row_c.clone();
        fd.select_folder(Some(&win_c), gio::Cancellable::NONE, move |r| {
            if let Ok(f) = r {
                if let Some(p) = f.path() {
                    row.set_subtitle(&p.display().to_string());
                }
            }
        });
    });

    general.add(&dl_group);

    let about_page = adw::PreferencesPage::builder()
        .title("About")
        .icon_name("help-about-symbolic")
        .build();

    let about_group = adw::PreferencesGroup::new();
    let app_name = gtk::Label::builder()
        .label("Rill")
        .css_classes(["title-1"])
        .build();
    let version = gtk::Label::builder()
        .label("Version 0.1.0")
        .css_classes(["dim-label"])
        .build();
    let desc = gtk::Label::builder()
        .label("Minimalistic BitTorrent client for GNOME")
        .css_classes(["dim-label"])
        .max_width_chars(40)
        .wrap(true)
        .build();
    let about_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .margin_top(24)
        .margin_bottom(24)
        .build();
    about_box.append(&app_name);
    about_box.append(&version);
    about_box.append(&desc);
    about_group.add(&about_box);
    about_page.add(&about_group);

    win.add(&general);
    win.add(&about_page);
    win.present();
}

// ── Helpers ──
fn fallback_name(uri: &str) -> String {
    if let Some(start) = uri.find("dn=") {
        let after = &uri[start + 3..];
        let raw = after.find('&').map(|e| &after[..e]).unwrap_or(after);
        return urldecode(raw);
    }
    if let Some(pos) = uri.rfind('/') {
        return uri[pos + 1..].to_string();
    }
    uri.to_string()
}

fn urldecode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'+' { out.push(' '); i += 1; }
        else if bytes[i] == b'%' && i + 2 < bytes.len() {
            let h = std::str::from_utf8(&bytes[i+1..i+3]).ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok());
            if let Some(c) = h { out.push(c as char); i += 3; }
            else { out.push(bytes[i] as char); i += 1; }
        } else { out.push(bytes[i] as char); i += 1; }
    }
    out
}

fn fmt_bytes(bytes: u64) -> String {
    const U: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut s = bytes as f64;
    let mut i = 0;
    while s >= 1024. && i < U.len() - 1 { s /= 1024.; i += 1; }
    if i == 0 { format!("{} B", bytes) } else { format!("{:.1} {}", s, U[i]) }
}

fn fmt_speed(bps: u64) -> String {
    if bps == 0 { return "0 B/s".into(); }
    if bps >= 1048576 { return format!("{:.1} MB/s", bps as f64 / 1048576.); }
    if bps >= 1024 { return format!("{:.0} KB/s", bps as f64 / 1024.); }
    format!("{} B/s", bps)
}
