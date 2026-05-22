use std::cell::RefCell;
use std::rc::Rc;
use std::path::Path;

use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;

use crate::engine::{TorrentEngine, TorrentUiState, UiUpdate};

mod imp {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, Default)]
    pub struct RillInfoDialog {
        pub title_lbl: RefCell<Option<gtk::Label>>,
        
        // Overview Tab Widgets
        pub state_lbl: RefCell<Option<gtk::Label>>,
        pub progress_bar: RefCell<Option<gtk::ProgressBar>>,
        pub size_lbl: RefCell<Option<gtk::Label>>,
        pub speed_down_lbl: RefCell<Option<gtk::Label>>,
        pub speed_up_lbl: RefCell<Option<gtk::Label>>,
        pub peers_lbl: RefCell<Option<gtk::Label>>,
        pub eta_lbl: RefCell<Option<gtk::Label>>,
        pub source_lbl: RefCell<Option<gtk::Label>>,
        pub save_path_lbl: RefCell<Option<gtk::Label>>,

        // List Boxes for other tabs
        pub files_list_box: RefCell<Option<gtk::ListBox>>,
        pub peers_list_box: RefCell<Option<gtk::ListBox>>,
        pub trackers_list_box: RefCell<Option<gtk::ListBox>>,

        // Control and status state
        pub files_populated: RefCell<bool>,
        pub trackers_populated: RefCell<bool>,
        pub shared_update: RefCell<Option<Rc<RefCell<Option<UiUpdate>>>>>,
        pub updating_ui: RefCell<bool>,
        pub last_sequential_toggle: RefCell<Option<std::time::Instant>>,

        // Action widgets
        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub sequential_switch: RefCell<Option<adw::SwitchRow>>,
        pub storage: RefCell<Option<crate::storage::Storage>>,
    }

    #[glib::object_subclass]
    impl glib::subclass::types::ObjectSubclass for RillInfoDialog {
        const NAME: &'static str = "RillInfoDialog";
        type Type = super::RillInfoDialog;
        type ParentType = adw::Window;
    }

    impl glib::subclass::object::ObjectImpl for RillInfoDialog {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Window properties (fallback; overridden by parent window in show_info_dialog)
            obj.set_default_width(360);
            obj.set_default_height(400);
            obj.set_modal(true);
            obj.set_resizable(true);

            // Title label (bold, centered or standard header title)
            let title_lbl = gtk::Label::builder()
                .css_classes(["title-2"])
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(25)
                .build();

            // View Stack and Switcher
            let view_stack = adw::ViewStack::builder()
                .vexpand(true)
                .hexpand(true)
                .build();

            let view_switcher = adw::ViewSwitcher::builder()
                .stack(&view_stack)
                .build();

            let header_bar = adw::HeaderBar::builder()
                .title_widget(&view_switcher)
                .build();

            // --- 1. OVERVIEW PAGE ---
            let overview_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
            overview_box.set_margin_top(16);
            overview_box.set_margin_bottom(16);
            overview_box.set_margin_start(16);
            overview_box.set_margin_end(16);

            let status_header_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
            status_header_box.set_halign(gtk::Align::Center);
            status_header_box.set_margin_bottom(12);

            let state_lbl = gtk::Label::builder()
                .css_classes(["title-4", "bold"])
                .halign(gtk::Align::Center)
                .build();

            let size_lbl = gtk::Label::builder()
                .css_classes(["body", "dim-label"])
                .halign(gtk::Align::Center)
                .build();

            status_header_box.append(&state_lbl);
            status_header_box.append(&size_lbl);

            let progress_bar = gtk::ProgressBar::builder()
                .show_text(false)
                .hexpand(true)
                .css_classes(["thin"])
                .build();

            let progress_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
            progress_box.append(&progress_bar);

            let progress_clamp = adw::Clamp::builder()
                .maximum_size(288)
                .child(&progress_box)
                .margin_bottom(12)
                .build();

            let details_group = adw::PreferencesGroup::builder()
                .title("Details")
                .build();

            let speed_down_lbl = gtk::Label::builder().css_classes(["body", "dim-label"]).build();
            let speed_down_row = adw::ActionRow::builder()
                .title("Download Speed")
                .build();
            speed_down_row.add_suffix(&speed_down_lbl);
            details_group.add(&speed_down_row);

            let speed_up_lbl = gtk::Label::builder().css_classes(["body", "dim-label"]).build();
            let speed_up_row = adw::ActionRow::builder()
                .title("Upload Speed")
                .build();
            speed_up_row.add_suffix(&speed_up_lbl);
            details_group.add(&speed_up_row);

            let peers_lbl = gtk::Label::builder().css_classes(["body", "dim-label"]).build();
            let peers_row = adw::ActionRow::builder()
                .title("Peers Count")
                .build();
            peers_row.add_suffix(&peers_lbl);
            details_group.add(&peers_row);

            let eta_lbl = gtk::Label::builder().css_classes(["body", "dim-label"]).build();
            let eta_row = adw::ActionRow::builder()
                .title("Estimated Time (ETA)")
                .build();
            eta_row.add_suffix(&eta_lbl);
            details_group.add(&eta_row);

            let paths_group = adw::PreferencesGroup::builder()
                .title("Storage and Source")
                .build();

            let source_lbl = gtk::Label::builder()
                .css_classes(["body", "caption", "dim-label"])
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .selectable(true)
                .max_width_chars(35)
                .build();
            let source_row = adw::ActionRow::builder()
                .title("Source URI")
                .build();
            source_row.add_suffix(&source_lbl);
            paths_group.add(&source_row);

            let save_path_lbl = gtk::Label::builder()
                .css_classes(["body", "caption", "dim-label"])
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .selectable(true)
                .max_width_chars(35)
                .build();
            let save_path_row = adw::ActionRow::builder()
                .title("Save Folder")
                .build();
            save_path_row.add_suffix(&save_path_lbl);
            paths_group.add(&save_path_row);

            let sequential_switch = adw::SwitchRow::builder()
                .title("Download from start to finish (sequential)")
                .active(false)
                .build();

            let obj_weak_seq = obj.downgrade();
            sequential_switch.connect_active_notify(move |sw| {
                if let Some(obj) = obj_weak_seq.upgrade() {
                    if *obj.imp().updating_ui.borrow() {
                        return;
                    }
                    obj.imp().last_sequential_toggle.replace(Some(std::time::Instant::now()));
                    let is_active = sw.is_active();
                    let info_hash_opt = obj.imp().shared_update.borrow().as_ref()
                        .and_then(|shared| shared.borrow().as_ref().map(|upd| upd.info_hash.clone()));
                    if let Some(info_hash) = info_hash_opt {
                        if let Some(engine) = obj.imp().engine.borrow().as_ref() {
                            engine.borrow().set_sequential(&info_hash, is_active);
                        }
                        if let Some(storage) = obj.imp().storage.borrow().as_ref()
                            && let Err(e) = storage.update_torrent_sequential(&info_hash, is_active)
                        {
                            log::warn!("Failed to update sequential flag in DB: {}", e);
                        }
                    }
                }
            });

            let options_group = adw::PreferencesGroup::builder()
                .title("Options")
                .build();
            options_group.add(&sequential_switch);

            overview_box.append(&status_header_box);
            overview_box.append(&progress_clamp);
            overview_box.append(&details_group);
            overview_box.append(&options_group);
            overview_box.append(&paths_group);

            let overview_scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&overview_box)
                .build();

            view_stack.add_titled_with_icon(
                &overview_scroll,
                Some("overview"),
                "Overview",
                "dialog-information-symbolic",
            );

            // --- 2. FILES PAGE ---
            let files_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
            files_box.set_margin_top(12);
            files_box.set_margin_bottom(12);
            files_box.set_margin_start(12);
            files_box.set_margin_end(12);

            let files_group = adw::PreferencesGroup::builder()
                .title("Contents")
                .build();

            let files_list_box = gtk::ListBox::builder()
                .css_classes(["boxed-list"])
                .selection_mode(gtk::SelectionMode::None)
                .build();
            files_group.add(&files_list_box);
            files_box.append(&files_group);

            let files_scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&files_box)
                .build();

            view_stack.add_titled_with_icon(
                &files_scroll,
                Some("files"),
                "Files",
                "folder-symbolic",
            );

            // --- 3. PEERS PAGE ---
            let peers_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
            peers_box.set_margin_top(12);
            peers_box.set_margin_bottom(12);
            peers_box.set_margin_start(12);
            peers_box.set_margin_end(12);

            let peers_group = adw::PreferencesGroup::builder()
                .title("Connected Peers")
                .build();

            let peers_list_box = gtk::ListBox::builder()
                .css_classes(["boxed-list"])
                .selection_mode(gtk::SelectionMode::None)
                .build();
            
            // Empty placeholder for peers
            let peers_empty = gtk::Label::builder()
                .label("No active peers connected.")
                .css_classes(["dim-label"])
                .halign(gtk::Align::Center)
                .margin_top(24)
                .build();
            peers_list_box.set_placeholder(Some(&peers_empty));

            peers_group.add(&peers_list_box);
            peers_box.append(&peers_group);

            let peers_scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&peers_box)
                .build();

            view_stack.add_titled_with_icon(
                &peers_scroll,
                Some("peers"),
                "Peers",
                "avatar-default-symbolic",
            );

            // --- 4. TRACKERS PAGE ---
            let trackers_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
            trackers_box.set_margin_top(12);
            trackers_box.set_margin_bottom(12);
            trackers_box.set_margin_start(12);
            trackers_box.set_margin_end(12);

            let trackers_group = adw::PreferencesGroup::builder()
                .title("Trackers")
                .build();

            let trackers_list_box = gtk::ListBox::builder()
                .css_classes(["boxed-list"])
                .selection_mode(gtk::SelectionMode::None)
                .build();
            trackers_group.add(&trackers_list_box);
            trackers_box.append(&trackers_group);

            let trackers_scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&trackers_box)
                .build();

            view_stack.add_titled_with_icon(
                &trackers_scroll,
                Some("trackers"),
                "Trackers",
                "network-transmit-receive-symbolic",
            );

            // Setup main window structure
            let toolbar_view = adw::ToolbarView::builder().build();
            toolbar_view.add_top_bar(&header_bar);
            toolbar_view.set_content(Some(&view_stack));

            obj.set_content(Some(&toolbar_view));

            // Store references
            self.title_lbl.replace(Some(title_lbl));
            self.state_lbl.replace(Some(state_lbl));
            self.progress_bar.replace(Some(progress_bar));
            self.size_lbl.replace(Some(size_lbl));
            self.speed_down_lbl.replace(Some(speed_down_lbl));
            self.speed_up_lbl.replace(Some(speed_up_lbl));
            self.peers_lbl.replace(Some(peers_lbl));
            self.eta_lbl.replace(Some(eta_lbl));
            self.source_lbl.replace(Some(source_lbl));
            self.save_path_lbl.replace(Some(save_path_lbl));

            self.files_list_box.replace(Some(files_list_box));
            self.peers_list_box.replace(Some(peers_list_box));
            self.trackers_list_box.replace(Some(trackers_list_box));
            self.sequential_switch.replace(Some(sequential_switch));
        }
    }

    impl gtk::subclass::widget::WidgetImpl for RillInfoDialog {}
    impl gtk::subclass::window::WindowImpl for RillInfoDialog {}
    impl adw::subclass::prelude::AdwWindowImpl for RillInfoDialog {}
}

glib::wrapper! {
    pub struct RillInfoDialog(ObjectSubclass<imp::RillInfoDialog>)
        @extends adw::Window, gtk::Window, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

struct TorrentFileInfo {
    path: String,
    size: u64,
}

impl RillInfoDialog {
    fn title_lbl(&self) -> gtk::Label {
        self.imp().title_lbl.borrow().clone().unwrap()
    }

    fn state_lbl(&self) -> gtk::Label {
        self.imp().state_lbl.borrow().clone().unwrap()
    }

    fn progress_bar(&self) -> gtk::ProgressBar {
        self.imp().progress_bar.borrow().clone().unwrap()
    }

    fn size_lbl(&self) -> gtk::Label {
        self.imp().size_lbl.borrow().clone().unwrap()
    }

    fn speed_down_lbl(&self) -> gtk::Label {
        self.imp().speed_down_lbl.borrow().clone().unwrap()
    }

    fn speed_up_lbl(&self) -> gtk::Label {
        self.imp().speed_up_lbl.borrow().clone().unwrap()
    }

    fn peers_lbl(&self) -> gtk::Label {
        self.imp().peers_lbl.borrow().clone().unwrap()
    }

    fn eta_lbl(&self) -> gtk::Label {
        self.imp().eta_lbl.borrow().clone().unwrap()
    }

    fn source_lbl(&self) -> gtk::Label {
        self.imp().source_lbl.borrow().clone().unwrap()
    }

    fn save_path_lbl(&self) -> gtk::Label {
        self.imp().save_path_lbl.borrow().clone().unwrap()
    }

    fn files_list_box(&self) -> gtk::ListBox {
        self.imp().files_list_box.borrow().clone().unwrap()
    }

    fn peers_list_box(&self) -> gtk::ListBox {
        self.imp().peers_list_box.borrow().clone().unwrap()
    }

    fn trackers_list_box(&self) -> gtk::ListBox {
        self.imp().trackers_list_box.borrow().clone().unwrap()
    }

    pub fn set_engine(&self, engine: Rc<RefCell<TorrentEngine>>) {
        *self.imp().engine.borrow_mut() = Some(engine);
    }

    fn sequential_switch(&self) -> adw::SwitchRow {
        self.imp().sequential_switch.borrow().clone().unwrap()
    }

    pub fn set_storage(&self, storage: crate::storage::Storage) {
        *self.imp().storage.borrow_mut() = Some(storage);
    }

    pub fn new(shared_update: Rc<RefCell<Option<UiUpdate>>>, name: &str) -> Self {
        let obj: Self = glib::Object::builder()
            .property("title", name)
            .build();

        obj.title_lbl().set_text(name);
        *obj.imp().shared_update.borrow_mut() = Some(shared_update.clone());

        if let Some(update) = shared_update.borrow().as_ref() {
            obj.apply_update(update);
        }

        let obj_weak = obj.downgrade();
        glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if let Some(obj) = obj_weak.upgrade() {
                if let Some(shared_ref) = obj.imp().shared_update.borrow().as_ref()
                    && let Some(update) = shared_ref.borrow().as_ref()
                {
                    obj.apply_update(update);
                }
                glib::ControlFlow::Continue
            } else {
                glib::ControlFlow::Break
            }
        });

        obj
    }

    pub fn apply_update(&self, update: &UiUpdate) {
        let last_toggle_opt = *self.imp().last_sequential_toggle.borrow();
        let ignore_sequential_update = if let Some(last_toggle) = last_toggle_opt {
            if last_toggle.elapsed() < std::time::Duration::from_millis(2000) {
                if update.sequential == self.sequential_switch().is_active() {
                    self.imp().last_sequential_toggle.replace(None);
                    false
                } else {
                    true
                }
            } else {
                self.imp().last_sequential_toggle.replace(None);
                false
            }
        } else {
            false
        };

        if !ignore_sequential_update && self.sequential_switch().is_active() != update.sequential {
            self.imp().updating_ui.replace(true);
            self.sequential_switch().set_active(update.sequential);
            self.imp().updating_ui.replace(false);
        }
        let progress = if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        };

        let state_text = match update.state {
            TorrentUiState::Downloading => format!("Downloading ({:.1}%)", progress * 100.0),
            TorrentUiState::Paused => format!("Paused ({:.1}%)", progress * 100.0),
            TorrentUiState::Completed => "Completed".to_string(),
            TorrentUiState::Error => "Error".to_string(),
        };
        self.state_lbl().set_text(&state_text);

        self.progress_bar().set_fraction(progress);

        let size_text = if update.total > 0 {
            format!(
                "{} of {}",
                format_size(update.downloaded),
                format_size(update.total)
            )
        } else {
            format_size(update.downloaded)
        };
        self.size_lbl().set_text(&size_text);

        self.speed_down_lbl()
            .set_text(&format!("↓ {}", format_size(update.speed_down)));
        self.speed_up_lbl()
            .set_text(&format!("↑ {}", format_size(update.speed_up)));
        self.peers_lbl().set_text(&update.peers.to_string());
        
        // Format ETA
        let eta_text = if update.state == TorrentUiState::Downloading && update.speed_down > 0 {
            format_eta(update.downloaded, update.total, update.speed_down)
        } else if update.state == TorrentUiState::Completed {
            "Done".to_string()
        } else {
            "∞".to_string()
        };
        self.eta_lbl().set_text(&eta_text);

        self.source_lbl().set_text(&update.uri);
        self.save_path_lbl()
            .set_text(&update.output_dir.to_string_lossy());

        // Update Peers list
        self.update_peers_tab(&update.peers_list);

        // Populate Files tab once if metadata is downloaded
        if !*self.imp().files_populated.borrow() {
            let files = load_torrent_files(&update.uri, &update.output_dir);
            if !files.is_empty() {
                self.populate_files_tab(&files);
                *self.imp().files_populated.borrow_mut() = true;
            }
        }

        // Populate Trackers tab once if metadata is downloaded
        if !*self.imp().trackers_populated.borrow() {
            let trackers = load_trackers(&update.uri, &update.output_dir);
            if !trackers.is_empty() {
                self.populate_trackers_tab(&trackers);
                *self.imp().trackers_populated.borrow_mut() = true;
            }
        }
    }

    fn update_peers_tab(&self, peers: &[crate::engine::PeerInfo]) {
        let list_box = self.peers_list_box();
        
        // Clear current peers
        while let Some(row) = list_box.row_at_index(0) {
            list_box.remove(&row);
        }

        for peer in peers {
            let row = adw::ActionRow::builder()
                .title(&peer.address)
                .subtitle(&peer.client)
                .build();

            // Right side indicators (Speed down/up & Encryption status)
            let right_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            right_box.set_valign(gtk::Align::Center);

            if peer.speed_down > 0 {
                let speed_down_badge = gtk::Label::builder()
                    .label(format!("↓ {}", format_size(peer.speed_down)))
                    .css_classes(["dim-label", "caption"])
                    .build();
                right_box.append(&speed_down_badge);
            }

            if peer.speed_up > 0 {
                let speed_up_badge = gtk::Label::builder()
                    .label(format!("↑ {}", format_size(peer.speed_up)))
                    .css_classes(["dim-label", "caption"])
                    .build();
                right_box.append(&speed_up_badge);
            }

            if peer.encrypted {
                let lock_img = gtk::Image::builder()
                    .icon_name("security-high-symbolic")
                    .css_classes(["accent-color"])
                    .build();
                right_box.append(&lock_img);
            }

            row.add_suffix(&right_box);
            list_box.append(&row);
        }
    }

    fn populate_files_tab(&self, files: &[TorrentFileInfo]) {
        let list_box = self.files_list_box();
        
        // Clear first
        while let Some(row) = list_box.row_at_index(0) {
            list_box.remove(&row);
        }

        for file in files {
            let checkbox = gtk::CheckButton::builder()
                .active(true)
                .valign(gtk::Align::Center)
                .build();

            let row = adw::ActionRow::builder()
                .title(&file.path)
                .subtitle(format_size(file.size))
                .build();

            row.add_prefix(&checkbox);

            // Priority DropDown/Menu
            let priority_lbl = gtk::Label::builder()
                .label("Normal Priority")
                .css_classes(["dim-label", "caption"])
                .valign(gtk::Align::Center)
                .build();
            row.add_suffix(&priority_lbl);

            list_box.append(&row);
        }
    }

    fn populate_trackers_tab(&self, trackers: &[String]) {
        let list_box = self.trackers_list_box();

        // Clear first
        while let Some(row) = list_box.row_at_index(0) {
            list_box.remove(&row);
        }

        for tracker in trackers {
            let escaped_tracker = glib::markup_escape_text(tracker);
            let row = adw::ActionRow::builder()
                .title(escaped_tracker.as_str())
                .build();

            let status_badge = gtk::Label::builder()
                .label("Connected")
                .css_classes(["success", "caption", "bold"])
                .valign(gtk::Align::Center)
                .build();

            row.add_suffix(&status_badge);
            list_box.append(&row);
        }
    }

}

fn load_torrent_files(uri: &str, output_dir: &Path) -> Vec<TorrentFileInfo> {
    // 1. If uri is a file path, load directly
    let path = Path::new(uri);
    if path.exists() && path.is_file()
        && let Ok(meta) = mtorrent::utils::re_exports::mtorrent_core::input::Metainfo::from_file(path)
    {
        return extract_files_from_meta(&meta);
    }
    // 2. Scan output directory for any .torrent files matching the download folder
    if let Ok(entries) = std::fs::read_dir(output_dir) {
        for entry in entries.filter_map(Result::ok) {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("torrent")
                && let Ok(meta) = mtorrent::utils::re_exports::mtorrent_core::input::Metainfo::from_file(&p)
            {
                return extract_files_from_meta(&meta);
            }
        }
    }
    Vec::new()
}

fn extract_files_from_meta(meta: &mtorrent::utils::re_exports::mtorrent_core::input::Metainfo) -> Vec<TorrentFileInfo> {
    let mut files = Vec::new();
    if let Some(meta_files) = meta.files() {
        for (len, path) in meta_files {
            files.push(TorrentFileInfo {
                path: path.to_string_lossy().to_string(),
                size: len as u64,
            });
        }
    } else {
        let name = meta.name().unwrap_or("Unknown File").to_string();
        let len = meta.length().unwrap_or(0) as u64;
        files.push(TorrentFileInfo {
            path: name,
            size: len,
        });
    }
    files
}

fn load_trackers(uri: &str, output_dir: &Path) -> Vec<String> {
    let mut list = Vec::new();
    
    // Attempt to load trackers from the torrent file
    let path = Path::new(uri);
    let mut loaded_meta = None;
    if path.exists() && path.is_file() {
        if let Ok(meta) = mtorrent::utils::re_exports::mtorrent_core::input::Metainfo::from_file(path) {
            loaded_meta = Some(meta);
        }
    } else if let Ok(entries) = std::fs::read_dir(output_dir) {
        for entry in entries.filter_map(Result::ok) {
            let p = entry.path();
            if p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("torrent")
                && let Ok(meta) = mtorrent::utils::re_exports::mtorrent_core::input::Metainfo::from_file(&p)
            {
                loaded_meta = Some(meta);
                break;
            }
        }
    }

    if let Some(meta) = loaded_meta {
        if let Some(tracker) = meta.announce() {
            list.push(tracker.to_string());
        }
        if let Some(trackers) = meta.announce_list() {
            for tier in trackers {
                for tr in tier {
                    let tr_str = tr.to_string();
                    if !list.contains(&tr_str) {
                        list.push(tr_str);
                    }
                }
            }
        }
    }

    // Fallback trackers for magnet links if not yet downloaded
    if list.is_empty() && uri.starts_with("magnet:") {
        for pair in uri.split('&') {
            if let Some(value) = pair.strip_prefix("tr=")
                && let Ok(decoded) = urlencoding::decode(value)
            {
                let decoded_str = decoded.into_owned();
                if !list.contains(&decoded_str) {
                    list.push(decoded_str);
                }
            }
        }
    }

    list
}

fn format_eta(downloaded: u64, total: u64, speed_down: u64) -> String {
    if speed_down == 0 {
        return "∞".to_string();
    }
    if downloaded >= total {
        return "0s".to_string();
    }
    let remaining = total - downloaded;
    let eta_secs = remaining / speed_down;
    if eta_secs < 60 {
        format!("{}s", eta_secs)
    } else if eta_secs < 3600 {
        format!("{}m {}s", eta_secs / 60, eta_secs % 60)
    } else if eta_secs < 86400 {
        format!("{}h {}m", eta_secs / 3600, (eta_secs % 3600) / 60)
    } else {
        format!("{}d {}h", eta_secs / 86400, (eta_secs % 86400) / 3600)
    }
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    const THRESHOLD: f64 = 1024.0;

    if bytes == 0 {
        return "0 B".to_string();
    }

    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= THRESHOLD && unit_index < UNITS.len() - 1 {
        size /= THRESHOLD;
        unit_index += 1;
    }

    if unit_index == 0 || size >= 100.0 {
        format!("{:.0} {}", size, UNITS[unit_index])
    } else if size >= 10.0 {
        format!("{:.1} {}", size, UNITS[unit_index])
    } else {
        format!("{:.2} {}", size, UNITS[unit_index])
    }
}

