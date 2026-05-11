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

    // About page  
    let about_page = adw::PreferencesPage::builder()
        .title("About")
        .icon_name("help-about-symbolic")
        .build();

    let about_group = adw::PreferencesGroup::builder()
        .title("Rill")
        .build();

    let version_row = adw::ActionRow::builder()
        .title("Version")
        .subtitle("0.1.0")
        .build();
    about_group.add(&version_row);

    let desc_row = adw::ActionRow::builder()
        .title("Description")
        .subtitle("Minimalistic BitTorrent client for GNOME")
        .build();
    about_group.add(&desc_row);

    about_page.add(&about_group);
    prefs.add(&about_page);

    prefs.present();
}