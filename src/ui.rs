use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use glib::clone;

use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};

// ── CSS ──
const APP_CSS: &str = "
.bubble {
    background-color: alpha(@accent_bg_color, 0.15);
    color: @accent_color;
    min-width: 36px;
    min-height: 36px;
    border-radius: 18px;
    padding: 0;
}
.bubble.success {
    background-color: alpha(@success_color, 0.15);
    color: @success_color;
}
.bubble.error {
    background-color: alpha(@error_color, 0.15);
    color: @error_color;
}
.bubble.muted {
    background-color: alpha(@window_fg_color, 0.10);
    color: @window_fg_color;
    opacity: 0.7;
}
progressbar.thin > trough,
progressbar.thin > trough > progress {
    min-height: 4px;
    border-radius: 2px;
}
.torrent-row {
    padding: 12px;
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

    // Channel: engine → UI
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

    // App actions
    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(|_, _| {
        log::info!("Rill v0.1.0");
    });
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

    // ── Torrent groups ──
    let dl_group = adw::PreferencesGroup::builder()
        .title("Downloading")
        .visible(false)
        .build();
    let dl_list = make_listbox();
    dl_group.add(&dl_list);

    let pause_group = adw::PreferencesGroup::builder()
        .title("Paused")
        .visible(false)
        .build();
    let pause_list = make_listbox();
    pause_group.add(&pause_list);

    let done_group = adw::PreferencesGroup::builder()
        .title("Completed")
        .visible(false)
        .build();
    let done_list = make_listbox();
    done_group.add(&done_list);

    let prefs_page = adw::PreferencesPage::new();
    prefs_page.add(&dl_group);
    prefs_page.add(&pause_group);
    prefs_page.add(&done_group);

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&prefs_page)
        .build();

    // Stack: empty ↔ torrents
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&scroll, Some("torrents"));
    stack.set_visible_child_name("empty");

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&stack));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Rill")
        .default_width(540)
        .default_height(680)
        .content(&toolbar_view)
        .build();

    // ── Shared state: info_hash → row ──
    let rows: Rc<RefCell<HashMap<String, RillRow>>> = Rc::new(RefCell::new(HashMap::new()));

    // ── Add button ──
    let eng_add = engine.clone();
    add_btn.connect_clicked(clone!(
        #[weak] window,
        move |_| show_add_dialog(&window, eng_add.clone())
    ));

    // ── Process engine updates ──
    glib::spawn_future_local(clone!(
        #[strong] rows,
        #[strong(rename_to = dl)] dl_list,
        #[strong(rename_to = pl)] pause_list,
        #[strong(rename_to = cl)] done_list,
        #[strong(rename_to = dlg)] dl_group,
        #[strong(rename_to = plg)] pause_group,
        #[strong(rename_to = clg)] done_group,
        #[strong] stack,
        #[strong] engine,
        async move {
            while let Ok(event) = rx.recv().await {
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

                        let (target, target_grp) = match update.state {
                            TorrentUiState::Downloading => (&dl, &dlg),
                            TorrentUiState::Paused => (&pl, &plg),
                            TorrentUiState::Completed | TorrentUiState::Error => (&cl, &clg),
                        };

                        let cur = row.container.parent()
                            .and_then(|p| p.downcast::<gtk::ListBox>().ok());
                        if cur.as_ref() != Some(target) {
                            if let Some(lb) = cur { lb.remove(&row.container); }
                            target.append(&row.container);
                        }
                        target_grp.set_visible(true);
                        stack.set_visible_child_name("torrents");
                    }
                    UiEvent::Finished { info_hash, error } => {
                        if let Some(e) = error {
                            log::error!("Torrent {info_hash}: {e}");
                        }
                    }
                }
            }
        }
    ));

    window.present();
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
    info_hash: String,
    uri: String,
    output_dir: PathBuf,
}

impl RillRow {
    fn new(update: &UiUpdate, engine: &Rc<RefCell<TorrentEngine>>) -> Self {
        // ── Bubble icon ──
        let icon = gtk::Image::builder()
            .icon_name("folder-download-symbolic")
            .pixel_size(16)
            .build();
        let icon_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .css_classes(["bubble"])
            .build();
        icon_box.append(&icon);

        // ── Info column ──
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
            .spacing(4)
            .hexpand(true)
            .valign(gtk::Align::Center)
            .build();
        info_box.append(&name_lbl);
        info_box.append(&status_lbl);
        info_box.append(&progress);

        // ── Action button ──
        let action_btn = gtk::Button::builder()
            .icon_name("media-playback-pause-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();

        // ── Layout ──
        let row_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(12)
            .css_classes(["torrent-row"])
            .build();
        row_box.append(&icon_box);
        row_box.append(&info_box);
        row_box.append(&action_btn);

        let container = gtk::ListBoxRow::builder()
            .activatable(false)
            .selectable(false)
            .child(&row_box)
            .build();

        let initial_state = update.state.clone();
        let shared_state: Rc<Cell<TorrentUiState>> = Rc::new(Cell::new(initial_state.clone()));

        // ── Right-click context menu ──
        let gesture = gtk::GestureClick::new();
        gesture.set_button(3);
        let container_ctx = container.clone();
        let action_btn_ctx = action_btn.clone();
        let icon_ctx = icon.clone();
        let state_ctx = Rc::clone(&shared_state);
        let info_hash_ctx = update.info_hash.clone();
        let uri_ctx = update.uri.clone();
        let out_ctx = update.output_dir.clone();
        let eng_ctx = engine.clone();
        gesture.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            show_context_menu(
                &container_ctx, &action_btn_ctx, &icon_ctx,
                state_ctx.get(),
                eng_ctx.clone(),
                &info_hash_ctx, &uri_ctx, &out_ctx,
                x, y,
            );
        });
        container.add_controller(gesture);

        // ── Action button click ──
        let info_hash_btn = update.info_hash.clone();
        let uri_btn = update.uri.clone();
        let out_btn = update.output_dir.clone();
        let eng_btn = engine.clone();
        let icon_btn = icon.clone();
        let act_btn = action_btn.clone();
        let state_btn = Rc::clone(&shared_state);
        let container_btn = container.clone();
        action_btn.connect_clicked(move |_| {
            match state_btn.get() {
                TorrentUiState::Downloading => {
                    eng_btn.borrow().toggle(&info_hash_btn);
                    state_btn.set(TorrentUiState::Paused);
                    set_row_paused(&act_btn, &icon_btn);
                }
                TorrentUiState::Paused => {
                    eng_btn.borrow().start(uri_btn.clone(), out_btn.clone());
                    state_btn.set(TorrentUiState::Downloading);
                    set_row_downloading(&act_btn, &icon_btn);
                }
                TorrentUiState::Completed | TorrentUiState::Error => {
                    if let Some(p) = container_btn.parent() {
                        if let Ok(lb) = p.downcast::<gtk::ListBox>() {
                            lb.remove(&container_btn);
                        }
                    }
                }
            }
        });

        // Set initial visual state
        let row = Self {
            container,
            name_lbl,
            status_lbl,
            progress,
            icon,
            action_btn,
            state: Rc::clone(&shared_state),
            info_hash: update.info_hash.clone(),
            uri: update.uri.clone(),
            output_dir: update.output_dir.clone(),
        };
        row.update(update);
        row
    }

    fn update(&self, u: &UiUpdate) {
        let name = if u.name.is_empty() { fallback_name(&u.uri) } else { u.name.clone() };
        self.name_lbl.set_label(&name);

        if u.total > 0 {
            self.progress.set_fraction(u.downloaded as f64 / u.total as f64);
        } else {
            self.progress.pulse();
        }

        let status = match u.state {
            TorrentUiState::Completed => format!("Complete · {}", fmt_bytes(u.total)),
            TorrentUiState::Paused => format!("Paused · {} of {}", fmt_bytes(u.downloaded), fmt_bytes(u.total)),
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

        match u.state {
            TorrentUiState::Downloading => {
                set_row_downloading(&self.action_btn, &self.icon);
            }
            TorrentUiState::Paused => {
                set_row_paused(&self.action_btn, &self.icon);
            }
            TorrentUiState::Completed => {
                self.icon.set_icon_name(Some("emblem-ok-symbolic"));
                self.icon.set_css_classes(&["bubble", "success"]);
                self.action_btn.set_icon_name("user-trash-symbolic");
                self.progress.set_fraction(1.0);
            }
            TorrentUiState::Error => {
                self.icon.set_icon_name(Some("dialog-error-symbolic"));
                self.icon.set_css_classes(&["bubble", "error"]);
                self.action_btn.set_icon_name("user-trash-symbolic");
            }
        }
    }
}

fn set_row_downloading(btn: &gtk::Button, icon: &gtk::Image) {
    btn.set_icon_name("media-playback-pause-symbolic");
    icon.set_icon_name(Some("folder-download-symbolic"));
    icon.set_css_classes(&["bubble"]);
}

fn set_row_paused(btn: &gtk::Button, icon: &gtk::Image) {
    btn.set_icon_name("media-playback-start-symbolic");
    icon.set_icon_name(Some("media-playback-pause-symbolic"));
    icon.set_css_classes(&["bubble", "muted"]);
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

    let h_pause = info_hash.to_string();
    let eng_pause = engine.clone();
    let a_pause = action_btn.clone();
    let i_pause = icon.clone();
    let pause = gio::SimpleAction::new("pause", None);
    pause.connect_activate(move |_, _| {
        eng_pause.borrow().toggle(&h_pause);
        set_row_paused(&a_pause, &i_pause);
    });

    let u_resume = uri.to_string();
    let o_resume = output_dir.clone();
    let eng_resume = engine.clone();
    let a_resume = action_btn.clone();
    let i_resume = icon.clone();
    let resume = gio::SimpleAction::new("resume", None);
    resume.connect_activate(move |_, _| {
        eng_resume.borrow().start(u_resume.clone(), o_resume.clone());
        set_row_downloading(&a_resume, &i_resume);
    });

    let h_rm = info_hash.to_string();
    let p_rm = parent.clone();
    let eng_rm = engine.clone();
    let remove = gio::SimpleAction::new("remove", None);
    remove.connect_activate(move |_, _| {
        eng_rm.borrow().toggle(&h_rm);
        if let Some(p) = p_rm.parent() {
            if let Ok(lb) = p.downcast::<gtk::ListBox>() { lb.remove(&p_rm); }
        }
    });

    match state {
        TorrentUiState::Downloading => { group.add_action(&pause); group.add_action(&remove); }
        TorrentUiState::Paused => { group.add_action(&resume); group.add_action(&remove); }
        TorrentUiState::Completed | TorrentUiState::Error => { group.add_action(&remove); }
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

    // Wiring
    entry.connect_changed(clone!(#[weak] add_btn, move |e| {
        add_btn.set_sensitive(!e.text().trim().is_empty());
    }));

    cancel_btn.connect_clicked(clone!(#[weak] dlg, move |_| dlg.close()));

    let eng_add = engine.clone();
    add_btn.connect_clicked(clone!(
        #[weak] dlg,
        #[weak] entry,
        #[strong] out_dir,
        move |_| {
            let uri = entry.text().to_string();
            eng_add.borrow().start(uri, out_dir.borrow().clone());
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
