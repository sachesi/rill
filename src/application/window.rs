use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gio, glib};
use glib::clone;

use async_channel::Sender;
use crate::engine::{TorrentEngine, TorrentUiState, UiEvent, UiUpdate};
use crate::model::TorrentModel;

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

pub struct RillWindow {
    window: adw::ApplicationWindow,
    engine: Rc<RefCell<TorrentEngine>>,
    model: Rc<RefCell<TorrentModel>>,
    
    // UI components
    empty: adw::StatusPage,
    dl_list: gtk::ListBox,
    pause_list: gtk::ListBox, 
    done_list: gtk::ListBox,
    dl_header: gtk::Label,
    pause_header: gtk::Label,
    done_header: gtk::Label,
    
    // Data tracking
    rows: Rc<RefCell<HashMap<String, RillRow>>>,
    tx: Sender<UiEvent>,
}

impl RillWindow {
    pub fn new(
        app: &adw::Application,
        engine: Rc<RefCell<TorrentEngine>>,
        model: Rc<RefCell<TorrentModel>>,
    ) -> Self {
        // Load CSS
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

        // Build window
        let window_self = Self::build_window(app, engine.clone(), model.clone(), tx.clone());
        
        // Set up update loop
        window_self.setup_update_loop(rx);
        
        window_self
    }

    fn build_window(
        app: &adw::Application,
        engine: Rc<RefCell<TorrentEngine>>,
        model: Rc<RefCell<TorrentModel>>,
        tx: Sender<UiEvent>,
    ) -> Self {
        // Header bar
        let title = adw::WindowTitle::new("Rill", "");

        let add_btn = gtk::Button::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("Add Torrent")
            .build();

        let main_menu = gio::Menu::new();
        main_menu.append(Some("_Preferences"), Some("app.preferences"));
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

        // Empty state
        let empty = adw::StatusPage::builder()
            .icon_name("folder-download-symbolic")
            .title("No Torrents")
            .description("Add a magnet link or .torrent file to get started")
            .vexpand(true)
            .hexpand(true)
            .build();

        // Content sections
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

        let rows = Rc::new(RefCell::new(HashMap::new()));

        // Add button click
        add_btn.connect_clicked(clone!(
            #[weak] window,
            #[strong] engine,
            #[strong] tx,
            move |_| show_add_dialog(&window, engine.clone(), tx.clone())
        ));

        Self {
            window,
            engine,
            model,
            empty,
            dl_list,
            pause_list,
            done_list,
            dl_header,
            pause_header,
            done_header,
            rows,
            tx,
        }
    }

    fn setup_update_loop(&self, rx: async_channel::Receiver<UiEvent>) {
        let rx_cell = Rc::new(RefCell::new(rx));
        
        glib::timeout_add_local(Duration::from_millis(200), clone!(
            #[strong(rename_to = model)] self.model,
            #[strong(rename_to = rows)] self.rows,
            #[strong(rename_to = dl)] self.dl_list,
            #[strong(rename_to = pl)] self.pause_list,
            #[strong(rename_to = cl)] self.done_list,
            #[strong(rename_to = dlh)] self.dl_header,
            #[strong(rename_to = ph)] self.pause_header,
            #[strong(rename_to = ch)] self.done_header,
            #[strong(rename_to = empty)] self.empty,
            #[strong(rename_to = engine)] self.engine,
            #[strong(rename_to = tx)] self.tx,
            #[strong] rx_cell,
            move || {
                let rx = rx_cell.borrow();
                while let Ok(event) = rx.try_recv() {
                    match event {
                        UiEvent::Update(update) => {
                            // Update the model first
                            model.borrow_mut().update_torrent(update.clone());
                            
                            // Update UI rows
                            let mut map = rows.borrow_mut();
                            let row = if let Some(r) = map.get(&update.info_hash) {
                                r.clone()
                            } else {
                                let r = RillRow::new(&update, &engine, tx.clone());
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
                                dlh.set_visible(false);
                                dl.set_visible(false);
                                ph.set_visible(false);
                                pl.set_visible(false);
                                ch.set_visible(false);
                                cl.set_visible(false);
                            }
                        }
                    }
                }
                glib::ControlFlow::Continue
            }
        ));
    }

    pub fn present(&self) {
        self.window.present();
    }
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

// Include RillRow implementation from ui.rs (temporarily)
// This will be moved to a separate module later
use std::cell::Cell;
use std::path::PathBuf;

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
    fn new(update: &UiUpdate, engine: &Rc<RefCell<TorrentEngine>>, tx: Sender<UiEvent>) -> Self {
        // For now, include basic implementation
        // Full implementation will be moved from ui.rs
        let icon = gtk::Image::builder()
            .icon_name("folder-download-symbolic")
            .pixel_size(24)
            .css_classes(["torrent-icon"])
            .margin_top(4)
            .margin_bottom(4)
            .build();

        let container = gtk::ListBoxRow::builder()
            .activatable(false)
            .selectable(false)
            .build();

        Self {
            container,
            name_lbl: gtk::Label::new(None),
            status_lbl: gtk::Label::new(None), 
            progress: gtk::ProgressBar::new(),
            icon,
            action_btn: gtk::Button::new(),
            state: Rc::new(Cell::new(update.state.clone())),
        }
    }

    fn update(&self, _update: &UiUpdate) {
        // Stub implementation - will be filled from ui.rs
    }
}

// Stub functions - will be moved from ui.rs
fn show_add_dialog(_window: &adw::ApplicationWindow, _engine: Rc<RefCell<TorrentEngine>>, _tx: Sender<UiEvent>) {
    log::info!("Add dialog requested - stub");
}