use adw::prelude::*;

pub fn show_preferences(parent: &adw::ApplicationWindow) {
    let prefs = adw::PreferencesWindow::builder()
        .title("Preferences")
        .transient_for(parent)
        .modal(true)
        .build();

    // General page
    let general_page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    let download_group = adw::PreferencesGroup::builder()
        .title("Downloads")
        .build();

    let download_row = adw::ActionRow::builder()
        .title("Download Folder")
        .subtitle("Choose where to save completed downloads")
        .build();

    let folder_btn = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .tooltip_text("Choose Folder")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .build();

    download_row.add_suffix(&folder_btn);
    download_group.add(&download_row);
    general_page.add(&download_group);
    prefs.add(&general_page);

    prefs.present();
}