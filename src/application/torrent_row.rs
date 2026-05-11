use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::{gio, glib};
use gtk::subclass::prelude::*;

use async_channel::Sender;

use super::info_dialog::RillInfoDialog;
use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(resource = "/com/github/sachesi/rill/ui/torrent-row.ui")]
    pub struct RillRow {
        #[template_child]
        pub icon: TemplateChild<gtk::Image>,
        #[template_child]
        pub name_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub status_lbl: TemplateChild<gtk::Label>,
        #[template_child]
        pub progress: TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub action_btn: TemplateChild<gtk::Button>,

        pub info_hash: RefCell<String>,
        pub state: RefCell<TorrentUiState>,
        pub engine: RefCell<Option<Rc<RefCell<TorrentEngine>>>>,
        pub tx: RefCell<Option<Sender<UiEvent>>>,
        pub latest_update: RefCell<Option<UiUpdate>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for RillRow {
        const NAME: &'static str = "RillRow";
        type Type = super::RillRow;
        type ParentType = gtk::ListBoxRow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[gtk::template_callbacks]
    impl RillRow {
    }

    impl ObjectImpl for RillRow {}
    impl WidgetImpl for RillRow {}
    impl ListBoxRowImpl for RillRow {}
}

glib::wrapper! {
    pub struct RillRow(ObjectSubclass<imp::RillRow>)
        @extends gtk::ListBoxRow, gtk::Widget,
        @implements gtk::Accessible, gtk::Actionable, gtk::Buildable, gtk::ConstraintTarget;
}

impl RillRow {
    pub fn new(
        info_hash: String,
        engine: Rc<RefCell<TorrentEngine>>,
        tx: Sender<UiEvent>,
    ) -> Self {
        let row: Self = glib::Object::builder().build();

        let imp = row.imp();
        imp.info_hash.replace(info_hash);
        imp.engine.replace(Some(engine));
        imp.tx.replace(Some(tx));

        // Right-click context menu
        let gesture = gtk::GestureClick::builder()
            .button(3)
            .build();
        let row_weak = row.downgrade();
        gesture.connect_pressed(move |_gesture, _n_press, _x, _y| {
            if let Some(row) = row_weak.upgrade() {
                show_context_menu(&row);
            }
        });
        row.add_controller(gesture);

        // Action button
        let row_weak = row.downgrade();
        let action_btn = imp.action_btn.clone();
        action_btn.connect_clicked(move |_| {
            if let Some(row) = row_weak.upgrade() {
                row.on_action();
            }
        });

        // Left-click → info dialog
        let left_gesture = gtk::GestureClick::builder()
            .button(1)
            .build();
        let row_weak2 = row.downgrade();
        left_gesture.connect_pressed(move |_gesture, _n_press, _x, _y| {
            if let Some(r) = row_weak2.upgrade() {
                if let Some(update) = r.imp().latest_update.borrow().as_ref() {
                    if let Some(root) = r.root() {
                        if let Ok(window) = root.downcast::<gtk::Window>() {
                            let dialog = RillInfoDialog::new(
                                update,
                                Rc::new(r.imp().latest_update.clone()),
                                &window,
                            );
                            dialog.present();
                        }
                    }
                }
            }
        });
        row.add_controller(left_gesture);

        row
    }

    pub fn info_hash(&self) -> String {
        self.imp().info_hash.borrow().clone()
    }

    pub fn state(&self) -> TorrentUiState {
        *self.imp().state.borrow()
    }

    pub fn name(&self) -> String {
        self.imp().name_lbl.text().to_string()
    }

    fn on_action(&self) {
        let imp = self.imp();
        let info_hash = imp.info_hash.borrow().clone();
        let state = *imp.state.borrow();
        if let Some(engine) = imp.engine.borrow().as_ref() {
            match state {
                TorrentUiState::Downloading | TorrentUiState::Paused => {
                    engine.borrow().toggle(&info_hash);
                }
                TorrentUiState::Completed | TorrentUiState::Error => {
                    engine.borrow().stop(&info_hash);
                }
            }
        }
    }

    pub fn update(&self, update: &UiUpdate) {
        let imp = self.imp();
        imp.latest_update.replace(Some(update.clone()));

        let name = if update.name.is_empty() {
            format!("Torrent {}", &update.info_hash[..8])
        } else {
            update.name.clone()
        };
        imp.name_lbl.set_text(&name);

        let speed = if update.speed_down > 0 {
            format!(" · {}↓", format_size(update.speed_down))
        } else {
            String::new()
        };
        let status = format!(
            "{} of {}{} · {} peers",
            format_size(update.downloaded),
            format_size(update.total),
            speed,
            update.peers
        );
        imp.status_lbl.set_text(&status);

        let fraction = if update.total > 0 {
            update.downloaded as f64 / update.total as f64
        } else {
            0.0
        };
        imp.progress.set_fraction(fraction);

        *imp.state.borrow_mut() = update.state;
        match update.state {
            TorrentUiState::Downloading => {
                imp.icon.set_icon_name(Some("folder-download-symbolic"));
                imp.icon.set_css_classes(&["torrent-icon", "accent"]);
                imp.action_btn.set_icon_name("media-playback-pause-symbolic");
                imp.action_btn.set_tooltip_text(Some("Pause"));
            }
            TorrentUiState::Paused => {
                imp.icon.set_icon_name(Some("media-playback-pause-symbolic"));
                imp.icon.set_css_classes(&["torrent-icon", "dim"]);
                imp.action_btn.set_icon_name("media-playback-start-symbolic");
                imp.action_btn.set_tooltip_text(Some("Resume"));
            }
            TorrentUiState::Completed => {
                imp.icon.set_icon_name(Some("emblem-ok-symbolic"));
                imp.icon.set_css_classes(&["torrent-icon", "success"]);
                imp.action_btn.set_icon_name("user-trash-symbolic");
                imp.action_btn.set_tooltip_text(Some("Remove"));
            }
            TorrentUiState::Error => {
                imp.icon.set_icon_name(Some("dialog-error-symbolic"));
                imp.icon.set_css_classes(&["torrent-icon", "error"]);
                imp.action_btn.set_icon_name("user-trash-symbolic");
                imp.action_btn.set_tooltip_text(Some("Remove"));
            }
        }
    }
}

fn show_context_menu(row: &RillRow) {
    let imp = row.imp();
    let info_hash = imp.info_hash.borrow().clone();
    let state = *imp.state.borrow();
    let engine = imp.engine.borrow().as_ref().cloned();
    let _tx = imp.tx.borrow().clone();

    let menu = gio::Menu::new();
    let actions = gio::SimpleActionGroup::new();

    match state {
        TorrentUiState::Downloading => {
            menu.append(Some("_Pause"), Some("ctx.pause"));
        }
        TorrentUiState::Paused => {
            menu.append(Some("_Resume"), Some("ctx.resume"));
        }
        TorrentUiState::Completed | TorrentUiState::Error => {}
    }
    menu.append(Some("_Remove"), Some("ctx.remove"));

    let popover = gtk::PopoverMenu::builder()
        .menu_model(&menu)
        .has_arrow(false)
        .build();
    popover.set_parent(row);

    // Pause action
    if let Some(eng) = engine.clone() {
        let h = info_hash.clone();
        let p = popover.clone();
        let a = gio::SimpleAction::new("pause", None);
        a.connect_activate(move |_, _| {
            eng.borrow().toggle(&h);
            p.popdown();
        });
        actions.add_action(&a);
    }

    // Resume action
    if let Some(eng) = engine.clone() {
        let h = info_hash.clone();
        let p = popover.clone();
        let a = gio::SimpleAction::new("resume", None);
        a.connect_activate(move |_, _| {
            eng.borrow().toggle(&h);
            p.popdown();
        });
        actions.add_action(&a);
    }

    // Remove action
    if let Some(eng) = engine {
        let h = info_hash.clone();
        let p = popover.clone();
        let a = gio::SimpleAction::new("remove", None);
        a.connect_activate(move |_, _| {
            eng.borrow().stop(&h);
            p.popdown();
        });
        actions.add_action(&a);
    }

    popover.insert_action_group("ctx", Some(&actions));
    popover.popup();
}

fn format_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit_index = 0;

    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{:.1} {}", size, UNITS[unit_index])
    }
}
