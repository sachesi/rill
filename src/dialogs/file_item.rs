use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gdk, gio, glib, pango};
use mtorrent::utils::re_exports::mtorrent_base::input::Metainfo;

use crate::util::format_size;

mod imp {
    use super::*;

    /// A file of a torrent, as the Files page lists it.
    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::FileItem)]
    pub struct FileItem {
        /// Where the file is in the torrent's own order.
        #[property(get, construct_only)]
        index: Cell<u32>,
        #[property(get, construct_only)]
        path: RefCell<String>,
        #[property(get, construct_only)]
        size: Cell<u64>,
        #[property(get, construct_only)]
        size_text: RefCell<String>,
        /// Whether the file is to be downloaded.
        #[property(get, set)]
        selected: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileItem {
        const NAME: &'static str = "RillFileItem";
        type Type = super::FileItem;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FileItem {}
}

glib::wrapper! {
    pub struct FileItem(ObjectSubclass<imp::FileItem>);
}

impl FileItem {
    pub fn new(index: usize, path: &str, size: u64, selected: bool) -> Self {
        glib::Object::builder()
            .property("index", index as u32)
            .property("path", path)
            .property("size", size)
            .property("size-text", format_size(size))
            .property("selected", selected)
            .build()
    }
}

/// Rows for a list of [`FileItem`]s: a check to download the file, with its path, and its
/// size beside it.
pub fn factory() -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, list_item| {
        let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        list_item.set_activatable(false);
        let path = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(pango::EllipsizeMode::Middle)
            .build();
        let check = gtk::CheckButton::builder()
            .hexpand(true)
            .child(&path)
            .build();
        // A child of its own is not taken as the check's name the way a label is.
        check.update_relation(&[gtk::accessible::Relation::LabelledBy(&[path.upcast_ref()])]);
        let size = gtk::Label::builder()
            .css_classes(["caption", "dim-label", "numeric"])
            .build();
        let row = gtk::Box::builder()
            .spacing(12)
            .margin_top(8)
            .margin_bottom(8)
            .margin_start(6)
            .margin_end(6)
            .build();
        row.append(&check);
        row.append(&size);
        list_item.set_child(Some(&row));

        let binding: RefCell<Option<glib::Binding>> = RefCell::default();
        list_item.connect_item_notify(move |list_item| {
            if let Some(binding) = binding.take() {
                binding.unbind();
            }
            let Some(item) = list_item.item().and_downcast::<FileItem>() else {
                return;
            };
            path.set_label(&item.path());
            size.set_label(&item.size_text());
            let selected = item
                .bind_property("selected", &check, "active")
                .bidirectional()
                .sync_create()
                .build();
            binding.replace(Some(selected));
        });
    });
    factory
}

/// The files of a torrent, path and size, in the torrent's own order: a single-file torrent
/// has one, named after the torrent.
pub fn torrent_files(meta: &Metainfo) -> Vec<(String, u64)> {
    match meta.files() {
        Some(files) => files
            .map(|(len, path)| (path.to_string_lossy().into_owned(), len as u64))
            .collect(),
        None => vec![(
            meta.name().unwrap_or_default().to_string(),
            meta.length().unwrap_or(0) as u64,
        )],
    }
}

thread_local! {
    /// Set while every check of a list is being moved at once, which is one change rather
    /// than one per file.
    static SETTING_ALL: Cell<bool> = const { Cell::new(false) };
}

/// Fills `store` with `files` (path and size), the `selected` ones checked, or all of them
/// with `None`, and calls `changed` whenever the check of one moves.
pub fn fill(
    store: &gio::ListStore,
    files: &[(String, u64)],
    selected: Option<&[usize]>,
    changed: impl Fn() + Clone + 'static,
) {
    let items: Vec<FileItem> = files
        .iter()
        .enumerate()
        .map(|(index, (path, size))| {
            let checked = selected.is_none_or(|selected| selected.contains(&index));
            let item = FileItem::new(index, path, *size, checked);
            let changed = changed.clone();
            item.connect_selected_notify(move |_| {
                if !SETTING_ALL.get() {
                    changed();
                }
            });
            item
        })
        .collect();
    store.splice(0, store.n_items(), &items);
}

/// Gives `view` a context menu, on a right click, a long press or the menu key, to check or
/// uncheck every file in `store` at once; `changed` is called once each time.
pub fn add_selection_menu(
    view: &gtk::ListView,
    store: &gio::ListStore,
    changed: impl Fn() + Clone + 'static,
) {
    let actions = gio::SimpleActionGroup::new();
    for (name, selected) in [("select-all", true), ("deselect-all", false)] {
        let action = gio::SimpleAction::new(name, None);
        let store = store.downgrade();
        let changed = changed.clone();
        action.connect_activate(move |_, _| {
            let Some(store) = store.upgrade() else {
                return;
            };
            SETTING_ALL.set(true);
            for item in items(&store) {
                item.set_selected(selected);
            }
            SETTING_ALL.set(false);
            changed();
        });
        actions.add_action(&action);
    }
    view.insert_action_group("files", Some(&actions));

    let menu = gio::Menu::new();
    menu.append(Some(&gettext("Select All")), Some("files.select-all"));
    menu.append(Some(&gettext("Deselect All")), Some("files.deselect-all"));
    let popover = gtk::PopoverMenu::from_model(Some(&menu));
    popover.set_has_arrow(false);
    popover.set_halign(gtk::Align::Start);
    popover.set_parent(view);
    view.connect_destroy(glib::clone!(
        #[weak]
        popover,
        move |_| popover.unparent()
    ));
    let open_at = glib::clone!(
        #[weak]
        popover,
        move |x: f64, y: f64| {
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        }
    );

    let click = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .build();
    let open = open_at.clone();
    click.connect_pressed(move |gesture, _, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        open(x, y);
    });
    view.add_controller(click);

    let press = gtk::GestureLongPress::builder().touch_only(true).build();
    let open = open_at.clone();
    press.connect_pressed(move |gesture, x, y| {
        gesture.set_state(gtk::EventSequenceState::Claimed);
        open(x, y);
    });
    view.add_controller(press);

    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Shift>F10|Menu"),
        Some(gtk::CallbackAction::new(move |view, _| {
            open_at(0.0, f64::from(view.height()) / 2.0);
            glib::Propagation::Stop
        })),
    ));
    view.add_controller(keys);
}

/// The indices of the files in `store` checked for download, or `None` when all of them are.
pub fn selected_files(store: &gio::ListStore) -> Option<Vec<usize>> {
    let selected: Vec<usize> = items(store)
        .filter(FileItem::selected)
        .map(|item| item.index() as usize)
        .collect();
    (selected.len() != store.n_items() as usize).then_some(selected)
}

/// How many bytes the files checked in `store` take together.
pub fn selected_size(store: &gio::ListStore) -> u64 {
    items(store)
        .filter(FileItem::selected)
        .map(|item| item.size())
        .fold(0, u64::saturating_add)
}

fn items(store: &gio::ListStore) -> impl Iterator<Item = FileItem> + '_ {
    store.iter::<FileItem>().filter_map(Result::ok)
}
