use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gettextrs::gettext;
use gtk::subclass::prelude::*;
use gtk::{gio, glib};

use async_channel::Sender;

use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};
use crate::util::format_size;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct RillRow {
        pub icon: RefCell<Option<gtk::Image>>,
        pub row_box: RefCell<Option<gtk::Box>>,
        pub name_lbl: RefCell<Option<gtk::Label>>,
        pub status_lbl: RefCell<Option<gtk::Label>>,
        pub progress: RefCell<Option<gtk::ProgressBar>>,
        pub action_btn: RefCell<Option<gtk::Button>>,
        pub action_icon: RefCell<Option<gtk::Image>>,
        pub check_btn: RefCell<Option<gtk::CheckButton>>,

        pub info_hash: RefCell<String>,
        pub state: RefCell<TorrentUiState>,
        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub tx: RefCell<Option<Sender<UiEvent>>>,
        pub latest_update: Rc<RefCell<Option<UiUpdate>>>,
        pub active_popover: RefCell<Option<gtk::PopoverMenu>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillRow {
        const NAME: &'static str = "RillRow";
        type Type = super::RillRow;
        type ParentType = gtk::ListBoxRow;
    }

    impl ObjectImpl for RillRow {
        fn constructed(&self) {
            self.parent_constructed();

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

            let info_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
            info_box.set_hexpand(true);
            info_box.set_valign(gtk::Align::Center);
            info_box.append(&name_lbl);
            info_box.append(&status_lbl);
            info_box.append(&progress);

            let icon = gtk::Image::builder()
                .icon_name("go-down-symbolic")
                .pixel_size(16)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .css_classes(["torrent-icon-avatar", "accent"])
                .build();
            icon.set_size_request(40, 40);

            let action_icon = gtk::Image::builder()
                .icon_name("media-playback-pause-symbolic")
                .pixel_size(16)
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build();

            let action_btn = gtk::Button::builder()
                .child(&action_icon)
                .valign(gtk::Align::Center)
                .halign(gtk::Align::Center)
                .css_classes(["circular"])
                .build();
            action_btn.set_size_request(36, 36);

            let check_btn = gtk::CheckButton::builder()
                .valign(gtk::Align::Center)
                .halign(gtk::Align::Center)
                .visible(false)
                .build();
            check_btn.set_size_request(36, 36);

            let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row_box.set_margin_top(10);
            row_box.set_margin_bottom(10);
            row_box.set_margin_start(12);
            row_box.set_margin_end(8);
            row_box.append(&icon);
            row_box.append(&info_box);
            row_box.append(&action_btn);
            row_box.append(&check_btn);

            self.obj().set_child(Some(&row_box));

            self.icon.replace(Some(icon));
            self.row_box.replace(Some(row_box));
            self.name_lbl.replace(Some(name_lbl));
            self.status_lbl.replace(Some(status_lbl));
            self.progress.replace(Some(progress));
            self.action_btn.replace(Some(action_btn));
            self.action_icon.replace(Some(action_icon));
            self.check_btn.replace(Some(check_btn));
        }

        fn dispose(&self) {
            if let Some(popover) = self.active_popover.borrow_mut().take() {
                popover.unparent();
            }
        }
    }
    impl WidgetImpl for RillRow {}
    impl ListBoxRowImpl for RillRow {}
}

glib::wrapper! {
    pub struct RillRow(ObjectSubclass<imp::RillRow>)
        @extends gtk::ListBoxRow, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl RillRow {
    fn name_lbl(&self) -> gtk::Label {
        self.imp().name_lbl.borrow().clone().unwrap()
    }

    fn status_lbl(&self) -> gtk::Label {
        self.imp().status_lbl.borrow().clone().unwrap()
    }

    fn progress(&self) -> gtk::ProgressBar {
        self.imp().progress.borrow().clone().unwrap()
    }

    fn icon(&self) -> gtk::Image {
        self.imp().icon.borrow().clone().unwrap()
    }

    fn row_box(&self) -> gtk::Box {
        self.imp().row_box.borrow().clone().unwrap()
    }

    fn action_btn(&self) -> gtk::Button {
        self.imp().action_btn.borrow().clone().unwrap()
    }

    fn action_icon(&self) -> gtk::Image {
        self.imp().action_icon.borrow().clone().unwrap()
    }

    fn check_btn(&self) -> gtk::CheckButton {
        self.imp().check_btn.borrow().clone().unwrap()
    }

    pub fn set_selection_mode(&self, active: bool) {
        let imp = self.imp();
        if let Some(btn) = imp.action_btn.borrow().as_ref() {
            btn.set_visible(!active);
        }
        if let Some(chk) = imp.check_btn.borrow().as_ref() {
            chk.set_visible(active);
        }
    }

    pub fn set_selected(&self, selected: bool) {
        if let Some(chk) = self.imp().check_btn.borrow().as_ref() {
            chk.set_active(selected);
        }
    }

    pub fn is_selected(&self) -> bool {
        self.imp()
            .check_btn
            .borrow()
            .as_ref()
            .is_some_and(|chk| chk.is_active())
    }

    pub fn new(info_hash: String, engine: Rc<RefCell<TorrentEngine>>, tx: Sender<UiEvent>) -> Self {
        let row: Self = glib::Object::builder().build();

        let imp = row.imp();
        imp.info_hash.replace(info_hash);
        imp.engine.replace(Some(engine));
        imp.tx.replace(Some(tx));

        // Right-click context menu
        let gesture = gtk::GestureClick::builder().button(3).build();
        let row_weak = row.downgrade();
        gesture.connect_pressed(move |_gesture, _n_press, x, y| {
            if let Some(row) = row_weak.upgrade() {
                show_context_menu(&row, x, y);
            }
        });
        row.add_controller(gesture);

        // Action button
        let row_weak = row.downgrade();
        let action_btn = row.action_btn();
        action_btn.connect_clicked(move |_| {
            if let Some(row) = row_weak.upgrade() {
                row.on_action();
            }
        });

        // Checkbox toggled
        let row_weak = row.downgrade();
        let check_btn = row.check_btn();
        check_btn.connect_toggled(move |chk| {
            if let Some(row) = row_weak.upgrade()
                && let Some(root) = row.root()
                && let Ok(window) = root.downcast::<crate::application::RillWindow>()
            {
                window.on_row_selection_toggled(&row, chk.is_active());
            }
        });

        row
    }

    pub fn info_hash(&self) -> String {
        self.imp().info_hash.borrow().clone()
    }

    pub fn state(&self) -> TorrentUiState {
        *self.imp().state.borrow()
    }

    pub fn name(&self) -> String {
        self.name_lbl().text().to_string()
    }

    pub fn latest_update(&self) -> Rc<RefCell<Option<UiUpdate>>> {
        self.imp().latest_update.clone()
    }

    fn on_action(&self) {
        let imp = self.imp();
        let info_hash = imp.info_hash.borrow().clone();
        let state = *imp.state.borrow();
        if let Some(engine) = imp.engine.borrow().as_ref() {
            match state {
                TorrentUiState::Downloading => {
                    self.update_ui_state(TorrentUiState::Paused);
                    if let Some(tx) = imp.tx.borrow().as_ref()
                        && let Some(prev) = imp.latest_update.borrow().as_ref()
                    {
                        let mut prospective = prev.clone();
                        prospective.state = TorrentUiState::Paused;
                        prospective.speed_down = 0;
                        prospective.speed_up = 0;
                        let _ = tx.try_send(UiEvent::Update(prospective));
                    }
                    engine.borrow().toggle(&info_hash);
                }
                TorrentUiState::Paused => {
                    self.update_ui_state(TorrentUiState::Downloading);
                    if let Some(tx) = imp.tx.borrow().as_ref()
                        && let Some(prev) = imp.latest_update.borrow().as_ref()
                    {
                        let mut prospective = prev.clone();
                        prospective.state = TorrentUiState::Downloading;
                        let _ = tx.try_send(UiEvent::Update(prospective));
                    }
                    engine.borrow().toggle(&info_hash);
                }
                TorrentUiState::Completed | TorrentUiState::Error => {
                    if let Some(tx) = imp.tx.borrow().as_ref() {
                        let _ = tx.try_send(UiEvent::Finished {
                            info_hash,
                            error: Some("user_requested_removal".to_string()),
                        });
                    }
                }
            }
        }
    }

    pub fn update_ui_state(&self, state: TorrentUiState) {
        let imp = self.imp();
        *imp.state.borrow_mut() = state;
        match state {
            TorrentUiState::Downloading => {
                // Arrow-down icon in blue tinted circle
                self.icon().set_icon_name(Some("go-down-symbolic"));
                self.icon()
                    .set_css_classes(&["torrent-icon-avatar", "accent"]);
                // Neutral flat pause button
                self.action_icon()
                    .set_icon_name(Some("media-playback-pause-symbolic"));
                self.action_btn().set_css_classes(&["circular", "flat"]);
                self.action_btn().set_tooltip_text(Some("Pause"));
                // Full card at full opacity
                self.row_box().set_opacity(1.0);
            }
            TorrentUiState::Paused => {
                // Pause icon in gray circle
                self.icon()
                    .set_icon_name(Some("media-playback-pause-symbolic"));
                self.icon().set_css_classes(&["torrent-icon-avatar", "dim"]);
                // Neutral flat resume button
                self.action_icon()
                    .set_icon_name(Some("media-playback-start-symbolic"));
                self.action_btn().set_css_classes(&["circular", "flat"]);
                self.action_btn().set_tooltip_text(Some("Resume"));
                // Dim entire card
                self.row_box().set_opacity(0.5);
            }
            TorrentUiState::Completed => {
                // Checkmark in green circle
                self.icon().set_icon_name(Some("emblem-ok-symbolic"));
                self.icon()
                    .set_css_classes(&["torrent-icon-avatar", "success"]);
                // Trash button (seeding finished)
                self.action_icon()
                    .set_icon_name(Some("user-trash-symbolic"));
                self.action_btn().set_css_classes(&["circular", "flat"]);
                self.action_btn().set_tooltip_text(Some("Remove"));
                self.row_box().set_opacity(1.0);
            }
            TorrentUiState::Error => {
                self.icon().set_icon_name(Some("dialog-error-symbolic"));
                self.icon()
                    .set_css_classes(&["torrent-icon-avatar", "error"]);
                self.action_icon()
                    .set_icon_name(Some("user-trash-symbolic"));
                self.action_btn().set_css_classes(&["circular", "flat"]);
                self.action_btn().set_tooltip_text(Some("Remove"));
                self.row_box().set_opacity(1.0);
            }
        }
    }

    pub fn update(&self, update: &UiUpdate) {
        let imp = self.imp();
        imp.latest_update.replace(Some(update.clone()));

        let name = if update.name.is_empty() {
            gettext("Torrent {id}").replace("{id}", &update.info_hash[..8])
        } else {
            update.name.clone()
        };
        self.name_lbl().set_text(&name);

        let speed = if update.speed_down > 0 {
            format!(" · {}↓", format_size(update.speed_down))
        } else {
            String::new()
        };
        let status = gettext("{downloaded} of {total}{speed} · {peers} peers")
            .replace("{downloaded}", &format_size(update.downloaded))
            .replace("{total}", &format_size(update.total))
            .replace("{speed}", &speed)
            .replace("{peers}", &update.peers.to_string());
        self.status_lbl().set_text(&status);

        let fraction = if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        };
        self.progress().set_fraction(fraction);

        self.update_ui_state(update.state);
    }
}

fn show_context_menu(row: &RillRow, x: f64, y: f64) {
    let imp = row.imp();
    let info_hash = imp.info_hash.borrow().clone();
    let state = *imp.state.borrow();
    let engine = imp.engine.borrow().as_ref().cloned();
    let _tx = imp.tx.borrow().clone();

    // Directory holding this torrent's content: the per-torrent folder for
    // multi-file torrents, falling back to the download dir for single-file ones.
    let content_dir = imp.latest_update.borrow().as_ref().map(|u| {
        let dir = u.output_dir.join(&u.name);
        if dir.is_dir() {
            dir
        } else {
            u.output_dir.clone()
        }
    });

    let is_selection = if let Some(root) = row.root()
        && let Ok(window) = root.downcast::<crate::application::RillWindow>()
    {
        window.is_selection_mode()
    } else {
        false
    };

    let menu = gio::Menu::new();
    let actions = gio::SimpleActionGroup::new();

    if is_selection {
        menu.append(
            Some(gettext("Select _all").as_str()),
            Some("ctx.select_all"),
        );
        menu.append(
            Some(gettext("_Deselect all").as_str()),
            Some("ctx.deselect_all"),
        );
        menu.append(
            Some(gettext("_Start selected").as_str()),
            Some("ctx.start_selected"),
        );
        menu.append(
            Some(gettext("S_top selected").as_str()),
            Some("ctx.stop_selected"),
        );
        menu.append(
            Some(gettext("_Remove selected").as_str()),
            Some("ctx.remove_selected"),
        );

        if let Some(old_popover) = imp.active_popover.borrow_mut().take() {
            old_popover.unparent();
        }

        let popover = gtk::PopoverMenu::builder()
            .menu_model(&menu)
            .has_arrow(false)
            .build();
        popover.set_parent(row);

        let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        popover.set_pointing_to(Some(&rect));

        imp.active_popover.replace(Some(popover.clone()));

        let p_weak = popover.downgrade();
        let weak_row = row.downgrade();
        let a = gio::SimpleAction::new("select_all", None);
        a.connect_activate(move |_, _| {
            if let Some(r) = weak_row.upgrade()
                && let Some(root) = r.root()
                && let Ok(window) = root.downcast::<crate::application::RillWindow>()
            {
                window.select_all_torrents();
            }
            if let Some(p) = p_weak.upgrade() {
                p.popdown();
            }
        });
        actions.add_action(&a);

        let p_weak = popover.downgrade();
        let weak_row = row.downgrade();
        let a = gio::SimpleAction::new("deselect_all", None);
        a.connect_activate(move |_, _| {
            if let Some(r) = weak_row.upgrade()
                && let Some(root) = r.root()
                && let Ok(window) = root.downcast::<crate::application::RillWindow>()
            {
                window.deselect_all_torrents();
            }
            if let Some(p) = p_weak.upgrade() {
                p.popdown();
            }
        });
        actions.add_action(&a);

        let p_weak = popover.downgrade();
        let weak_row = row.downgrade();
        let a = gio::SimpleAction::new("start_selected", None);
        a.connect_activate(move |_, _| {
            if let Some(r) = weak_row.upgrade()
                && let Some(root) = r.root()
                && let Ok(window) = root.downcast::<crate::application::RillWindow>()
            {
                window.start_selected_torrents();
            }
            if let Some(p) = p_weak.upgrade() {
                p.popdown();
            }
        });
        actions.add_action(&a);

        let p_weak = popover.downgrade();
        let weak_row = row.downgrade();
        let a = gio::SimpleAction::new("stop_selected", None);
        a.connect_activate(move |_, _| {
            if let Some(r) = weak_row.upgrade()
                && let Some(root) = r.root()
                && let Ok(window) = root.downcast::<crate::application::RillWindow>()
            {
                window.stop_selected_torrents();
            }
            if let Some(p) = p_weak.upgrade() {
                p.popdown();
            }
        });
        actions.add_action(&a);

        let p_weak = popover.downgrade();
        let weak_row = row.downgrade();
        let a = gio::SimpleAction::new("remove_selected", None);
        a.connect_activate(move |_, _| {
            if let Some(r) = weak_row.upgrade()
                && let Some(root) = r.root()
                && let Ok(window) = root.downcast::<crate::application::RillWindow>()
            {
                window.remove_selected_torrents();
            }
            if let Some(p) = p_weak.upgrade() {
                p.popdown();
            }
        });
        actions.add_action(&a);

        popover.insert_action_group("ctx", Some(&actions));
        popover.popup();
    } else {
        match state {
            TorrentUiState::Downloading => {
                menu.append(Some(gettext("_Pause").as_str()), Some("ctx.pause"));
            }
            TorrentUiState::Paused => {
                menu.append(Some(gettext("_Resume").as_str()), Some("ctx.resume"));
            }
            TorrentUiState::Completed | TorrentUiState::Error => {}
        }
        if content_dir.is_some() {
            menu.append(Some(gettext("_Open").as_str()), Some("ctx.open"));
        }
        menu.append(Some(gettext("_Select").as_str()), Some("ctx.select"));
        menu.append(Some(gettext("_Remove").as_str()), Some("ctx.remove"));

        if let Some(old_popover) = imp.active_popover.borrow_mut().take() {
            old_popover.unparent();
        }

        let popover = gtk::PopoverMenu::builder()
            .menu_model(&menu)
            .has_arrow(false)
            .build();
        popover.set_parent(row);

        let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
        popover.set_pointing_to(Some(&rect));

        imp.active_popover.replace(Some(popover.clone()));

        let p_weak = popover.downgrade();
        if let Some(engine) = engine.clone() {
            let h = info_hash.clone();
            let r = row.clone();
            let a = gio::SimpleAction::new("pause", None);
            let p_weak_clone = p_weak.clone();
            a.connect_activate(move |_, _| {
                r.update_ui_state(TorrentUiState::Paused);
                if let Some(tx) = r.imp().tx.borrow().as_ref()
                    && let Some(prev) = r.imp().latest_update.borrow().as_ref()
                {
                    let mut prospective = prev.clone();
                    prospective.state = TorrentUiState::Paused;
                    prospective.speed_down = 0;
                    prospective.speed_up = 0;
                    let _ = tx.try_send(UiEvent::Update(prospective));
                }
                engine.borrow().toggle(&h);
                if let Some(p) = p_weak_clone.upgrade() {
                    p.popdown();
                }
            });
            actions.add_action(&a);
        }

        let p_weak = popover.downgrade();
        if let Some(engine) = engine.clone() {
            let h = info_hash.clone();
            let r = row.clone();
            let a = gio::SimpleAction::new("resume", None);
            let p_weak_clone = p_weak.clone();
            a.connect_activate(move |_, _| {
                r.update_ui_state(TorrentUiState::Downloading);
                if let Some(tx) = r.imp().tx.borrow().as_ref()
                    && let Some(prev) = r.imp().latest_update.borrow().as_ref()
                {
                    let mut prospective = prev.clone();
                    prospective.state = TorrentUiState::Downloading;
                    let _ = tx.try_send(UiEvent::Update(prospective));
                }
                engine.borrow().toggle(&h);
                if let Some(p) = p_weak_clone.upgrade() {
                    p.popdown();
                }
            });
            actions.add_action(&a);
        }

        let p_weak = popover.downgrade();
        if engine.is_some() {
            let h = info_hash.clone();
            let a = gio::SimpleAction::new("remove", None);
            let row_weak = row.downgrade();
            let p_weak_clone = p_weak.clone();
            a.connect_activate(move |_, _| {
                if let Some(row) = row_weak.upgrade()
                    && let Some(root) = row.root()
                    && let Ok(window) = root.downcast::<crate::application::RillWindow>()
                {
                    window.delete_torrent(&h);
                }
                if let Some(p) = p_weak_clone.upgrade() {
                    p.popdown();
                }
            });
            actions.add_action(&a);
        }

        if let Some(dir) = content_dir {
            let p_weak = popover.downgrade();
            let row_weak = row.downgrade();
            let a = gio::SimpleAction::new("open", None);
            a.connect_activate(move |_, _| {
                let file = gio::File::for_path(&dir);
                let launcher = gtk::FileLauncher::new(Some(&file));
                let parent = row_weak
                    .upgrade()
                    .and_then(|r| r.root())
                    .and_downcast::<gtk::Window>();
                launcher.launch(parent.as_ref(), gio::Cancellable::NONE, |res| {
                    if let Err(e) = res {
                        log::warn!("Failed to open torrent directory: {}", e);
                    }
                });
                if let Some(p) = p_weak.upgrade() {
                    p.popdown();
                }
            });
            actions.add_action(&a);
        }

        let p_weak = popover.downgrade();
        let weak_row = row.downgrade();
        let a = gio::SimpleAction::new("select", None);
        let p_weak_clone = p_weak.clone();
        a.connect_activate(move |_, _| {
            if let Some(r) = weak_row.upgrade()
                && let Some(root) = r.root()
                && let Ok(window) = root.downcast::<crate::application::RillWindow>()
            {
                window.enter_selection_mode();
                r.set_selected(true);
            }
            if let Some(p) = p_weak_clone.upgrade() {
                p.popdown();
            }
        });
        actions.add_action(&a);

        popover.insert_action_group("ctx", Some(&actions));
        popover.popup();
    }
}
