mod aethos_core;
mod relay;

use gtk4::gdk::Display;
use gtk4::prelude::*;
use gtk4::{
    glib, Application, ApplicationWindow, Box as GtkBox, Button, ComboBoxText, CssProvider, Dialog,
    Entry, Image, Label, ListBox, ListBoxRow, Orientation, Paned, ResponseType, Revealer,
    RevealerTransitionType, ScrolledWindow, Stack, StackSwitcher, TextView,
    STYLE_PROVIDER_PRIORITY_APPLICATION,
};
use image::{ImageBuffer, Luma, Rgba, RgbaImage};
use qrcode::QrCode;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::aethos_core::identity_store::{
    delete_wayfarer_id, ensure_local_identity, load_contact_aliases, load_relay_session_cache,
    regenerate_local_identity, save_contact_aliases, save_relay_session_cache, RelaySessionCache,
};
use crate::aethos_core::protocol::{build_envelope_payload_b64_from_utf8, is_valid_wayfarer_id};
use crate::relay::client::{
    connect_to_relay, connect_to_relay_with_auth, normalize_http_endpoint,
    send_to_relay_v1_with_auth, to_ws_endpoint, RelayFrame, RelayRequestDispatcher,
    RelaySessionConfig, RelaySessionManager,
};

const APP_ID: &str = "org.aethos.linux";
const DEFAULT_RELAY_HTTP_PRIMARY: &str = "http://192.168.1.200:8082";
const DEFAULT_RELAY_HTTP_SECONDARY: &str = "http://192.168.1.200:9082";
const APP_LOG_FILE_NAME: &str = "aethos-linux.log";
const CHAT_HISTORY_FILE_NAME: &str = "chat-history.json";
const SHARE_QR_FILE_NAME: &str = "share-wayfarer-qr.png";

#[derive(Clone, Debug)]
struct RelayStatus {
    relay_slot: usize,
    relay_http: String,
    relay_ws: String,
    state: String,
    dispatch: String,
}

#[derive(Clone, Copy, Debug)]
enum SessionOp {
    Send,
}

#[derive(Clone, Debug)]
struct SessionStatus {
    op: SessionOp,
    text: String,
    ack_msg_id: Option<String>,
    outgoing_contact: Option<String>,
    outgoing_text: Option<String>,
    pulled_messages: Vec<PulledMessagePreview>,
}

#[derive(Clone, Debug)]
struct PulledMessagePreview {
    from_wayfarer_id: String,
    msg_id: String,
    text: String,
    received_at: i64,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum ChatDirection {
    Incoming,
    Outgoing,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct ChatMessage {
    msg_id: String,
    text: String,
    timestamp: String,
    direction: ChatDirection,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct PersistedChatState {
    selected_contact: Option<String>,
    threads: BTreeMap<String, Vec<ChatMessage>>,
}

#[derive(Default, Debug)]
struct ChatState {
    selected_contact: Option<String>,
    threads: BTreeMap<String, Vec<ChatMessage>>,
    show_full_contact_id: bool,
    contact_aliases: BTreeMap<String, String>,
}

fn main() -> glib::ExitCode {
    if let Err(err) = ensure_linux_desktop_integration() {
        eprintln!("desktop integration warning: {err}");
    }

    let app = Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

fn build_ui(app: &Application) {
    apply_styles();

    let window = ApplicationWindow::builder()
        .application(app)
        .title("Aethos Chat · Linux")
        .default_width(980)
        .default_height(680)
        .build();
    window.set_icon_name(Some(APP_ID));

    let root = GtkBox::new(Orientation::Vertical, 12);
    root.add_css_class("root");
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_margin_top(20);
    root.set_margin_bottom(20);
    root.set_margin_start(20);
    root.set_margin_end(20);

    let tab_switcher = StackSwitcher::new();
    tab_switcher.set_halign(gtk4::Align::Start);
    tab_switcher.set_vexpand(false);

    let relay_chip = GtkBox::new(Orientation::Horizontal, 6);
    relay_chip.add_css_class("relay-chip");
    relay_chip.set_halign(gtk4::Align::End);
    relay_chip.set_hexpand(true);
    let relay_dot = Label::new(Some("*"));
    relay_dot.add_css_class("relay-dot");
    relay_dot.add_css_class("relay-dot-idle");
    let relay_chip_text = Label::new(Some("Relays: idle"));
    relay_chip_text.add_css_class("relay-chip-text");
    relay_chip.append(&relay_dot);
    relay_chip.append(&relay_chip_text);

    let top_bar = GtkBox::new(Orientation::Horizontal, 8);
    top_bar.set_hexpand(true);
    top_bar.append(&tab_switcher);
    top_bar.append(&relay_chip);

    let views = Stack::new();
    views.set_hexpand(true);
    views.set_vexpand(true);
    tab_switcher.set_stack(Some(&views));

    let onboarding_panel = GtkBox::new(Orientation::Vertical, 10);
    onboarding_panel.add_css_class("glass-panel");

    let onboarding_title = Label::new(Some("Onboarding"));
    onboarding_title.add_css_class("section-title");
    onboarding_title.set_xalign(0.0);

    let onboarding_status =
        Label::new(Some("Step 1/2 · Identity auto-provisions on first launch."));
    onboarding_status.set_xalign(0.0);
    onboarding_status.set_wrap(true);

    let id_box = GtkBox::new(Orientation::Horizontal, 8);
    let wayfarer_id_entry = Entry::builder().hexpand(true).editable(false).build();
    wayfarer_id_entry.set_placeholder_text(Some("No Wayfarer ID generated yet"));

    let identity_meta_label = Label::new(Some("Identity metadata: unavailable"));
    identity_meta_label.set_xalign(0.0);
    identity_meta_label.set_wrap(true);

    let generate_button = Button::with_label("Rotate Wayfarer ID");
    generate_button.add_css_class("action");
    let delete_button = Button::with_label("Reset Wayfarer ID");
    delete_button.add_css_class("danger");

    id_box.append(&wayfarer_id_entry);
    id_box.append(&generate_button);
    id_box.append(&delete_button);

    let identity_notice = Label::new(Some(
        "Your Wayfarer ID is your global address. Resetting it is destructive and can break contact reachability unless everyone learns your new ID.",
    ));
    identity_notice.add_css_class("warning");
    identity_notice.set_xalign(0.0);
    identity_notice.set_wrap(true);

    let proceed_button = Button::with_label("Open Settings");
    proceed_button.add_css_class("action");

    onboarding_panel.append(&onboarding_title);
    onboarding_panel.append(&onboarding_status);
    onboarding_panel.append(&id_box);
    onboarding_panel.append(&identity_meta_label);
    onboarding_panel.append(&identity_notice);
    onboarding_panel.append(&proceed_button);

    let diagnostics_panel = GtkBox::new(Orientation::Vertical, 10);
    diagnostics_panel.add_css_class("glass-panel");

    let relay_config_title = Label::new(Some("Relay diagnostics"));
    relay_config_title.add_css_class("section-title");
    relay_config_title.set_xalign(0.0);

    let relay_http_primary_entry = Entry::builder().hexpand(true).build();
    relay_http_primary_entry.set_text(DEFAULT_RELAY_HTTP_PRIMARY);
    let relay_http_secondary_entry = Entry::builder().hexpand(true).build();
    relay_http_secondary_entry.set_text(DEFAULT_RELAY_HTTP_SECONDARY);

    let connect_button = Button::with_label("Run Relay Diagnostics");
    connect_button.add_css_class("action");

    let open_logs_button = Button::with_label("Open Log Folder");
    open_logs_button.add_css_class("compact");

    let relay_primary_label = Label::new(Some("Primary relay status: idle"));
    relay_primary_label.set_xalign(0.0);
    relay_primary_label.set_wrap(true);
    let relay_secondary_label = Label::new(Some("Secondary relay status: idle"));
    relay_secondary_label.set_xalign(0.0);
    relay_secondary_label.set_wrap(true);

    let diagnostics_text = TextView::new();
    diagnostics_text.set_editable(false);
    diagnostics_text.set_cursor_visible(false);
    diagnostics_text.set_wrap_mode(gtk4::WrapMode::WordChar);
    diagnostics_text
        .buffer()
        .set_text("Diagnostics timeline:\n- waiting for first relay run");

    let diagnostics_scroll = ScrolledWindow::builder().min_content_height(160).build();
    diagnostics_scroll.set_child(Some(&diagnostics_text));

    diagnostics_panel.append(&relay_config_title);
    diagnostics_panel.append(&relay_http_primary_entry);
    diagnostics_panel.append(&relay_http_secondary_entry);
    diagnostics_panel.append(&connect_button);
    diagnostics_panel.append(&open_logs_button);
    diagnostics_panel.append(&relay_primary_label);
    diagnostics_panel.append(&relay_secondary_label);
    diagnostics_panel.append(&diagnostics_scroll);

    let conversations_panel = GtkBox::new(Orientation::Vertical, 10);
    conversations_panel.add_css_class("glass-panel");

    let conversations_title = Label::new(Some("Chats"));
    conversations_title.add_css_class("section-title");
    conversations_title.set_xalign(0.0);

    let conversations_hint = Label::new(Some(
        "Messages are grouped by wayfarer identity pairs. Select a contact to view the thread.",
    ));
    conversations_hint.set_xalign(0.0);

    conversations_panel.append(&conversations_title);
    conversations_panel.append(&conversations_hint);

    let chat_shell = Paned::new(Orientation::Horizontal);
    chat_shell.add_css_class("chat-shell");
    chat_shell.set_wide_handle(true);
    chat_shell.set_position(300);
    chat_shell.set_shrink_start_child(true);
    chat_shell.set_shrink_end_child(true);
    chat_shell.set_vexpand(true);

    let contacts_column = GtkBox::new(Orientation::Vertical, 8);
    contacts_column.add_css_class("contacts-pane");
    let contacts_title = Label::new(Some("Contacts"));
    contacts_title.add_css_class("section-title");
    contacts_title.set_xalign(0.0);
    let contacts_list = ListBox::new();
    contacts_list.add_css_class("contact-list");
    let contacts_scroll = ScrolledWindow::builder().min_content_width(260).build();
    contacts_scroll.set_vexpand(true);
    contacts_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    contacts_scroll.set_child(Some(&contacts_list));
    contacts_column.append(&contacts_title);
    contacts_column.append(&contacts_scroll);
    contacts_column.set_vexpand(true);
    let contacts_revealer = Revealer::builder()
        .transition_type(RevealerTransitionType::SlideRight)
        .transition_duration(180)
        .reveal_child(true)
        .build();
    contacts_revealer.set_child(Some(&contacts_column));

    let thread_column = GtkBox::new(Orientation::Vertical, 10);
    thread_column.add_css_class("thread-pane");
    let thread_title = Label::new(Some("Thread"));
    thread_title.add_css_class("section-title");
    thread_title.set_xalign(0.0);
    let thread_contact_id_label = Label::new(Some(""));
    thread_contact_id_label.add_css_class("thread-contact-id");
    thread_contact_id_label.set_xalign(0.0);
    let compact_contact_picker = ComboBoxText::new();
    compact_contact_picker.add_css_class("compact-contact-picker");
    let compact_picker_revealer = Revealer::builder()
        .transition_type(RevealerTransitionType::SlideDown)
        .transition_duration(180)
        .reveal_child(false)
        .build();
    compact_picker_revealer.set_child(Some(&compact_contact_picker));
    let id_toggle_button = Button::with_label("Show Full ID");
    id_toggle_button.add_css_class("compact");

    let thread_header = GtkBox::new(Orientation::Horizontal, 8);
    let thread_header_labels = GtkBox::new(Orientation::Vertical, 0);
    thread_header_labels.append(&thread_title);
    thread_header_labels.append(&thread_contact_id_label);
    thread_header_labels.append(&compact_picker_revealer);
    thread_header.append(&thread_header_labels);
    thread_header.append(&id_toggle_button);
    let messages_list = ListBox::new();
    messages_list.add_css_class("messages-list");
    let messages_scroll = ScrolledWindow::builder().build();
    messages_scroll.set_vexpand(true);
    messages_scroll.set_policy(gtk4::PolicyType::Never, gtk4::PolicyType::Automatic);
    messages_scroll.set_child(Some(&messages_list));
    thread_column.append(&thread_header);
    thread_column.append(&messages_scroll);

    thread_column.set_vexpand(true);

    chat_shell.set_start_child(Some(&contacts_revealer));
    chat_shell.set_end_child(Some(&thread_column));
    conversations_panel.append(&chat_shell);
    conversations_panel.set_vexpand(true);

    let recipient_entry = Entry::builder().hexpand(true).build();
    recipient_entry.set_placeholder_text(Some("Select a contact to start messaging"));
    recipient_entry.add_css_class("recipient-entry");
    recipient_entry.set_editable(false);

    let body_entry = Entry::builder().hexpand(true).build();
    body_entry.set_placeholder_text(Some("Type a message..."));
    body_entry.add_css_class("message-entry");

    let send_button = Button::with_label("↑");
    send_button.add_css_class("action");
    send_button.add_css_class("send-fab");

    let chat_status_label = Label::new(Some("Ready"));
    chat_status_label.add_css_class("chat-status");
    chat_status_label.set_xalign(0.0);
    chat_status_label.set_hexpand(true);

    let open_chat_logs_button = Button::with_label("Diagnostics Logs");
    open_chat_logs_button.add_css_class("compact");

    let composer_bar = GtkBox::new(Orientation::Horizontal, 8);
    composer_bar.add_css_class("composer-bar");
    composer_bar.append(&body_entry);
    composer_bar.append(&send_button);

    thread_column.append(&composer_bar);
    thread_column.append(&open_chat_logs_button);

    let contacts_panel = GtkBox::new(Orientation::Vertical, 10);
    contacts_panel.add_css_class("glass-panel");
    let contacts_manage_title = Label::new(Some("Contacts"));
    contacts_manage_title.add_css_class("section-title");
    contacts_manage_title.set_xalign(0.0);
    let contacts_manage_hint = Label::new(Some(
        "Add, rename, or remove contacts here. Chats only handles messages.",
    ));
    contacts_manage_hint.set_xalign(0.0);

    let contacts_manage_list = ListBox::new();
    contacts_manage_list.add_css_class("contact-list");
    let contacts_manage_scroll = ScrolledWindow::builder().min_content_height(280).build();
    contacts_manage_scroll.set_child(Some(&contacts_manage_list));
    contacts_manage_scroll.set_vexpand(true);

    let contact_id_entry = Entry::builder().hexpand(true).build();
    contact_id_entry.set_placeholder_text(Some("Wayfarer ID (64 lowercase hex)"));
    let contact_alias_entry = Entry::builder().hexpand(true).build();
    contact_alias_entry.set_placeholder_text(Some("Display name (optional, local only)"));

    let contacts_actions = GtkBox::new(Orientation::Horizontal, 8);
    let add_update_contact_button = Button::with_label("Add / Update Contact");
    add_update_contact_button.add_css_class("action");
    let remove_contact_button = Button::with_label("Remove Contact");
    remove_contact_button.add_css_class("danger");
    contacts_actions.append(&add_update_contact_button);
    contacts_actions.append(&remove_contact_button);

    contacts_panel.append(&contacts_manage_title);
    contacts_panel.append(&contacts_manage_hint);
    contacts_panel.append(&contacts_manage_scroll);
    contacts_panel.append(&contact_id_entry);
    contacts_panel.append(&contact_alias_entry);
    contacts_panel.append(&contacts_actions);

    let share_panel = GtkBox::new(Orientation::Vertical, 10);
    share_panel.add_css_class("glass-panel");
    let share_title = Label::new(Some("Share"));
    share_title.add_css_class("section-title");
    share_title.set_xalign(0.0);
    let share_hint = Label::new(Some(
        "Share your Wayfarer ID via QR. The code includes your address with Aethos feather mark.",
    ));
    share_hint.set_xalign(0.0);
    share_hint.set_wrap(true);
    let share_wayfarer_entry = Entry::builder().hexpand(true).editable(false).build();
    let copy_wayfarer_button = Button::with_label("Copy Wayfarer ID");
    copy_wayfarer_button.add_css_class("compact");
    let share_qr_image = Image::new();
    share_qr_image.set_pixel_size(280);
    share_qr_image.set_halign(gtk4::Align::Center);
    let share_status_label = Label::new(Some("QR pending identity"));
    share_status_label.set_xalign(0.0);
    share_status_label.add_css_class("chat-status");

    share_panel.append(&share_title);
    share_panel.append(&share_hint);
    share_panel.append(&share_wayfarer_entry);
    share_panel.append(&copy_wayfarer_button);
    share_panel.append(&share_qr_image);
    share_panel.append(&share_status_label);

    let settings_panel = GtkBox::new(Orientation::Vertical, 12);
    settings_panel.add_css_class("settings-shell");

    let account_group_title = Label::new(Some("Identity & Account"));
    account_group_title.add_css_class("settings-group-title");
    account_group_title.set_xalign(0.0);
    let account_group_hint = Label::new(Some(
        "Manage your local Wayfarer identity and safety-critical reset actions.",
    ));
    account_group_hint.add_css_class("settings-group-hint");
    account_group_hint.set_xalign(0.0);
    onboarding_panel.add_css_class("settings-card");

    let relay_group_title = Label::new(Some("Relay & Connectivity"));
    relay_group_title.add_css_class("settings-group-title");
    relay_group_title.set_xalign(0.0);
    let relay_group_hint = Label::new(Some(
        "Diagnostics and endpoint health for your configured relays.",
    ));
    relay_group_hint.add_css_class("settings-group-hint");
    relay_group_hint.set_xalign(0.0);
    diagnostics_panel.add_css_class("settings-card");

    settings_panel.append(&account_group_title);
    settings_panel.append(&account_group_hint);
    settings_panel.append(&onboarding_panel);
    settings_panel.append(&relay_group_title);
    settings_panel.append(&relay_group_hint);
    settings_panel.append(&diagnostics_panel);
    let settings_scroll = ScrolledWindow::builder().build();
    settings_scroll.add_css_class("settings-scroll");
    settings_scroll.set_child(Some(&settings_panel));
    settings_scroll.set_vexpand(true);

    views.add_titled(&conversations_panel, Some("sessions"), "Chats");
    views.add_titled(&contacts_panel, Some("contacts"), "Contacts");
    views.add_titled(&share_panel, Some("share"), "Share");
    views.add_titled(&settings_scroll, Some("settings"), "Settings");

    let footer = GtkBox::new(Orientation::Horizontal, 10);
    footer.add_css_class("footer-bar");
    footer.set_hexpand(true);
    footer.set_vexpand(false);
    footer.set_valign(gtk4::Align::End);
    footer.set_size_request(-1, 28);

    let footer_note = Label::new(Some("Aethos Linux app"));
    footer_note.add_css_class("footer-note");
    footer_note.set_xalign(0.0);
    footer_note.set_hexpand(true);

    let footer_logo = Image::from_file("src/img/logo.png");
    footer_logo.add_css_class("footer-logo");
    footer_logo.set_pixel_size(16);
    footer_logo.set_size_request(16, 16);
    footer_logo.set_hexpand(false);
    footer_logo.set_vexpand(false);
    footer_logo.set_halign(gtk4::Align::End);

    footer.append(&footer_note);
    footer.append(&footer_logo);

    root.append(&top_bar);
    root.append(&views);
    root.append(&footer);
    window.set_child(Some(&root));
    views.set_visible_child_name("sessions");

    if let Ok(identity) = ensure_local_identity() {
        wayfarer_id_entry.set_text(&identity.wayfarer_id);
        share_wayfarer_entry.set_text(&identity.wayfarer_id);
        refresh_share_qr(&identity.wayfarer_id, &share_qr_image, &share_status_label);
        let key_preview: String = identity.verifying_key_b64.chars().take(16).collect();
        let device_preview: String = identity.device_id.chars().take(12).collect();
        identity_meta_label.set_text(&format!(
            "Identity metadata: device={} · device_id={}… · verify_key={}…",
            identity.device_name, device_preview, key_preview
        ));
        onboarding_status
            .set_text("Step 2/2 · Identity provisioned. Proceed to relay diagnostics.");
    }

    {
        let share_wayfarer_entry = share_wayfarer_entry.clone();
        copy_wayfarer_button.connect_clicked(move |_| {
            if let Some(display) = Display::default() {
                display
                    .clipboard()
                    .set_text(&share_wayfarer_entry.text().to_string());
            }
        });
    }

    if let Ok(Some(cache)) = load_relay_session_cache() {
        relay_primary_label.set_text(&format!("Primary relay status: {}", cache.primary_status));
        relay_secondary_label.set_text(&format!(
            "Secondary relay status: {}",
            cache.secondary_status
        ));
        update_relay_chip(
            &cache.primary_status,
            &cache.secondary_status,
            &relay_dot,
            &relay_chip_text,
            &relay_chip,
        );
    } else {
        update_relay_chip("idle", "idle", &relay_dot, &relay_chip_text, &relay_chip);
    }

    let chat_state = Rc::new(RefCell::new(ChatState::default()));
    let contact_order = Rc::new(RefCell::new(Vec::<String>::new()));
    let contacts_manage_order = Rc::new(RefCell::new(Vec::<String>::new()));
    let picker_syncing = Rc::new(Cell::new(false));

    if let Ok(aliases) = load_contact_aliases() {
        chat_state.borrow_mut().contact_aliases = aliases;
    }

    if let Ok(Some(saved_chat)) = load_persisted_chat_state() {
        let mut state = chat_state.borrow_mut();
        state.threads = saved_chat.threads;
        state.selected_contact = saved_chat.selected_contact;
    }

    let first_contact = chat_state.borrow().contact_aliases.keys().next().cloned();
    if let Some(first_contact) = first_contact {
        if chat_state.borrow().selected_contact.is_none() {
            chat_state.borrow_mut().selected_contact = Some(first_contact.clone());
        }
        recipient_entry.set_text(&first_contact);
    }

    render_contacts(&chat_state.borrow(), &contacts_list, &contact_order);
    render_contacts_manager(
        &chat_state.borrow(),
        &contacts_manage_list,
        &contacts_manage_order,
    );
    picker_syncing.set(true);
    sync_contact_picker(
        &chat_state.borrow(),
        &compact_contact_picker,
        &contact_order,
    );
    picker_syncing.set(false);
    sync_contact_form(
        &chat_state.borrow(),
        &contact_id_entry,
        &contact_alias_entry,
    );
    render_messages(
        &chat_state.borrow(),
        &messages_list,
        &messages_scroll,
        &thread_title,
        &thread_contact_id_label,
    );

    {
        let chat_state = Rc::clone(&chat_state);
        let contact_order = Rc::clone(&contact_order);
        let messages_list = messages_list.clone();
        let messages_scroll = messages_scroll.clone();
        let thread_title = thread_title.clone();
        let thread_contact_id_label = thread_contact_id_label.clone();
        let recipient_entry = recipient_entry.clone();
        let compact_contact_picker = compact_contact_picker.clone();
        let contact_id_entry = contact_id_entry.clone();
        let contact_alias_entry = contact_alias_entry.clone();
        let picker_syncing = Rc::clone(&picker_syncing);
        contacts_list.connect_row_selected(move |_list, row| {
            let Some(row) = row else {
                return;
            };

            let idx = row.index();
            if idx < 0 {
                return;
            }

            let selected = contact_order.borrow().get(idx as usize).cloned();
            if let Some(contact_id) = selected {
                chat_state.borrow_mut().selected_contact = Some(contact_id);
                let _ = save_persisted_chat_state(&chat_state.borrow());
                if let Some(selected_contact) = chat_state.borrow().selected_contact.as_ref() {
                    recipient_entry.set_text(selected_contact);
                }
                picker_syncing.set(true);
                sync_contact_picker(
                    &chat_state.borrow(),
                    &compact_contact_picker,
                    &contact_order,
                );
                picker_syncing.set(false);
                sync_contact_form(
                    &chat_state.borrow(),
                    &contact_id_entry,
                    &contact_alias_entry,
                );
                render_messages(
                    &chat_state.borrow(),
                    &messages_list,
                    &messages_scroll,
                    &thread_title,
                    &thread_contact_id_label,
                );
            }
        });
    }

    {
        let chat_state = Rc::clone(&chat_state);
        let contact_order = Rc::clone(&contact_order);
        let messages_list = messages_list.clone();
        let messages_scroll = messages_scroll.clone();
        let thread_title = thread_title.clone();
        let thread_contact_id_label = thread_contact_id_label.clone();
        let recipient_entry = recipient_entry.clone();
        let contact_id_entry = contact_id_entry.clone();
        let contact_alias_entry = contact_alias_entry.clone();
        let picker_syncing = Rc::clone(&picker_syncing);
        compact_contact_picker.connect_changed(move |picker| {
            if picker_syncing.get() {
                return;
            }
            let Some(active_id) = picker.active_id() else {
                return;
            };
            let active_id = active_id.to_string();
            if !contact_order
                .borrow()
                .iter()
                .any(|contact| contact == &active_id)
            {
                return;
            }

            chat_state.borrow_mut().selected_contact = Some(active_id.clone());
            let _ = save_persisted_chat_state(&chat_state.borrow());
            recipient_entry.set_text(&active_id);
            sync_contact_form(
                &chat_state.borrow(),
                &contact_id_entry,
                &contact_alias_entry,
            );
            render_messages(
                &chat_state.borrow(),
                &messages_list,
                &messages_scroll,
                &thread_title,
                &thread_contact_id_label,
            );
        });
    }

    {
        let chat_state = Rc::clone(&chat_state);
        let contact_order = Rc::clone(&contact_order);
        let contacts_manage_order = Rc::clone(&contacts_manage_order);
        let contacts_list = contacts_list.clone();
        let contacts_manage_list = contacts_manage_list.clone();
        let compact_contact_picker = compact_contact_picker.clone();
        let contact_id_entry = contact_id_entry.clone();
        let contact_alias_entry = contact_alias_entry.clone();
        let chat_status_label = chat_status_label.clone();
        let picker_syncing = Rc::clone(&picker_syncing);
        let recipient_entry = recipient_entry.clone();
        add_update_contact_button.connect_clicked(move |_| {
            let contact_id = contact_id_entry.text().trim().to_string();
            if !is_valid_wayfarer_id(&contact_id) {
                chat_status_label
                    .set_text("invalid contact id: expected 64 lowercase hex characters");
                return;
            }

            let alias = contact_alias_entry.text().trim().to_string();
            {
                let mut state = chat_state.borrow_mut();
                state.contact_aliases.insert(contact_id.clone(), alias);
                state.selected_contact = Some(contact_id.clone());

                if let Err(err) = save_contact_aliases(&state.contact_aliases) {
                    chat_status_label.set_text(&format!("failed to save contact name: {err}"));
                    return;
                }
            }
            if let Err(err) = save_persisted_chat_state(&chat_state.borrow()) {
                chat_status_label.set_text(&format!("failed to persist chat state: {err}"));
                return;
            }

            render_contacts(&chat_state.borrow(), &contacts_list, &contact_order);
            render_contacts_manager(
                &chat_state.borrow(),
                &contacts_manage_list,
                &contacts_manage_order,
            );
            picker_syncing.set(true);
            sync_contact_picker(
                &chat_state.borrow(),
                &compact_contact_picker,
                &contact_order,
            );
            picker_syncing.set(false);
            sync_contact_form(
                &chat_state.borrow(),
                &contact_id_entry,
                &contact_alias_entry,
            );
            if let Some(selected) = chat_state.borrow().selected_contact.as_ref() {
                recipient_entry.set_text(selected);
            }
            chat_status_label.set_text("Contact saved locally");
        });
    }

    {
        let chat_state = Rc::clone(&chat_state);
        let contact_order = Rc::clone(&contact_order);
        let contacts_manage_order = Rc::clone(&contacts_manage_order);
        let contacts_list = contacts_list.clone();
        let contacts_manage_list = contacts_manage_list.clone();
        let compact_contact_picker = compact_contact_picker.clone();
        let contact_id_entry = contact_id_entry.clone();
        let contact_alias_entry = contact_alias_entry.clone();
        let chat_status_label = chat_status_label.clone();
        let picker_syncing = Rc::clone(&picker_syncing);
        let recipient_entry = recipient_entry.clone();
        remove_contact_button.connect_clicked(move |_| {
            let contact_id = contact_id_entry.text().trim().to_string();
            if contact_id.is_empty() {
                chat_status_label.set_text("Select a contact to remove");
                return;
            }

            {
                let mut state = chat_state.borrow_mut();
                state.contact_aliases.remove(&contact_id);
                state.threads.remove(&contact_id);
                if state.selected_contact.as_deref() == Some(contact_id.as_str()) {
                    state.selected_contact = state.contact_aliases.keys().next().cloned();
                }
                if let Err(err) = save_contact_aliases(&state.contact_aliases) {
                    chat_status_label.set_text(&format!("failed to remove contact: {err}"));
                    return;
                }
            }
            if let Err(err) = save_persisted_chat_state(&chat_state.borrow()) {
                chat_status_label.set_text(&format!("failed to persist chat state: {err}"));
                return;
            }

            render_contacts(&chat_state.borrow(), &contacts_list, &contact_order);
            render_contacts_manager(
                &chat_state.borrow(),
                &contacts_manage_list,
                &contacts_manage_order,
            );
            picker_syncing.set(true);
            sync_contact_picker(
                &chat_state.borrow(),
                &compact_contact_picker,
                &contact_order,
            );
            picker_syncing.set(false);
            sync_contact_form(
                &chat_state.borrow(),
                &contact_id_entry,
                &contact_alias_entry,
            );
            if let Some(selected) = chat_state.borrow().selected_contact.as_ref() {
                recipient_entry.set_text(selected);
            } else {
                recipient_entry.set_text("");
            }
            chat_status_label.set_text("Contact removed locally");
        });
    }

    {
        let add_update_contact_button = add_update_contact_button.clone();
        contact_id_entry.connect_activate(move |_| {
            add_update_contact_button.emit_clicked();
        });
    }

    {
        let add_update_contact_button = add_update_contact_button.clone();
        contact_alias_entry.connect_activate(move |_| {
            add_update_contact_button.emit_clicked();
        });
    }

    {
        let chat_state = Rc::clone(&chat_state);
        let contacts_manage_order = Rc::clone(&contacts_manage_order);
        let contact_order = Rc::clone(&contact_order);
        let contacts_list = contacts_list.clone();
        let contact_id_entry = contact_id_entry.clone();
        let contact_alias_entry = contact_alias_entry.clone();
        let recipient_entry = recipient_entry.clone();
        let compact_contact_picker = compact_contact_picker.clone();
        let picker_syncing = Rc::clone(&picker_syncing);
        let messages_list = messages_list.clone();
        let messages_scroll = messages_scroll.clone();
        let thread_title = thread_title.clone();
        let thread_contact_id_label = thread_contact_id_label.clone();
        let contacts_manage_list = contacts_manage_list.clone();
        contacts_manage_list.connect_row_selected(move |_list, row| {
            let Some(row) = row else {
                return;
            };

            let idx = row.index();
            if idx < 0 {
                return;
            }

            if let Some(contact_id) = contacts_manage_order.borrow().get(idx as usize).cloned() {
                chat_state.borrow_mut().selected_contact = Some(contact_id);
                let _ = save_persisted_chat_state(&chat_state.borrow());
                if let Some(selected) = chat_state.borrow().selected_contact.as_ref() {
                    recipient_entry.set_text(selected);
                }
                render_contacts(&chat_state.borrow(), &contacts_list, &contact_order);
                picker_syncing.set(true);
                sync_contact_picker(
                    &chat_state.borrow(),
                    &compact_contact_picker,
                    &contact_order,
                );
                picker_syncing.set(false);
                sync_contact_form(
                    &chat_state.borrow(),
                    &contact_id_entry,
                    &contact_alias_entry,
                );
                render_messages(
                    &chat_state.borrow(),
                    &messages_list,
                    &messages_scroll,
                    &thread_title,
                    &thread_contact_id_label,
                );
            }
        });
    }

    {
        let chat_state = Rc::clone(&chat_state);
        let messages_list = messages_list.clone();
        let messages_scroll = messages_scroll.clone();
        let thread_title = thread_title.clone();
        let thread_contact_id_label = thread_contact_id_label.clone();
        let id_toggle_button = id_toggle_button.clone();
        id_toggle_button.connect_clicked(move |button| {
            {
                let mut state = chat_state.borrow_mut();
                state.show_full_contact_id = !state.show_full_contact_id;
                if state.show_full_contact_id {
                    button.set_label("Show Short ID");
                } else {
                    button.set_label("Show Full ID");
                }
            }
            render_messages(
                &chat_state.borrow(),
                &messages_list,
                &messages_scroll,
                &thread_title,
                &thread_contact_id_label,
            );
        });
    }

    let (tx, rx) = channel::<RelayStatus>();

    {
        let window = window.clone();
        let wayfarer_id_entry = wayfarer_id_entry.clone();
        let identity_meta_label = identity_meta_label.clone();
        let onboarding_status = onboarding_status.clone();
        let share_wayfarer_entry = share_wayfarer_entry.clone();
        let share_qr_image = share_qr_image.clone();
        let share_status_label = share_status_label.clone();
        generate_button.connect_clicked(move |_| {
            let dialog = Dialog::builder()
                .transient_for(&window)
                .modal(true)
                .title("Rotate Wayfarer ID?")
                .build();
            dialog.add_button("Cancel", ResponseType::Cancel);
            dialog.add_button("Rotate ID", ResponseType::Accept);

            let content = dialog.content_area();
            let warning = Label::new(Some(
                "Rotating creates a new Wayfarer address. Existing contacts may not reach you until they update to your new ID.",
            ));
            warning.set_wrap(true);
            warning.set_xalign(0.0);
            warning.add_css_class("warning");
            content.append(&warning);

            let wayfarer_id_entry = wayfarer_id_entry.clone();
            let identity_meta_label = identity_meta_label.clone();
            let onboarding_status = onboarding_status.clone();
            let share_wayfarer_entry = share_wayfarer_entry.clone();
            let share_qr_image = share_qr_image.clone();
            let share_status_label = share_status_label.clone();
            dialog.connect_response(move |dialog, response| {
                if response == ResponseType::Accept {
                    match regenerate_local_identity() {
                        Ok(identity) => {
                            let key_preview: String =
                                identity.verifying_key_b64.chars().take(16).collect();
                            let device_preview: String = identity.device_id.chars().take(12).collect();
                            wayfarer_id_entry.set_text(&identity.wayfarer_id);
                            share_wayfarer_entry.set_text(&identity.wayfarer_id);
                            refresh_share_qr(
                                &identity.wayfarer_id,
                                &share_qr_image,
                                &share_status_label,
                            );
                            identity_meta_label.set_text(&format!(
                                "Identity metadata: device={} · device_id={}… · verify_key={}…",
                                identity.device_name, device_preview, key_preview
                            ));
                            onboarding_status.set_text(
                                "Step 2/2 · Identity rotated. Share your new Wayfarer ID with contacts.",
                            );
                        }
                        Err(err) => eprintln!("{err}"),
                    }
                }
                dialog.close();
            });
            dialog.present();
        });
    }

    {
        let window = window.clone();
        let wayfarer_id_entry = wayfarer_id_entry.clone();
        let identity_meta_label = identity_meta_label.clone();
        let onboarding_status = onboarding_status.clone();
        let share_wayfarer_entry = share_wayfarer_entry.clone();
        let share_qr_image = share_qr_image.clone();
        let share_status_label = share_status_label.clone();
        delete_button.connect_clicked(move |_| {
            let dialog = Dialog::builder()
                .transient_for(&window)
                .modal(true)
                .title("Reset Wayfarer ID?")
                .build();
            dialog.add_button("Cancel", ResponseType::Cancel);
            dialog.add_button("Reset ID", ResponseType::Accept);

            let content = dialog.content_area();
            let warning = Label::new(Some(
                "Your Wayfarer ID is your address. Resetting it is destructive: existing contacts will not know your new ID and may keep sending to the old one.",
            ));
            warning.set_wrap(true);
            warning.set_xalign(0.0);
            warning.add_css_class("warning");
            content.append(&warning);

            let wayfarer_id_entry = wayfarer_id_entry.clone();
            let identity_meta_label = identity_meta_label.clone();
            let onboarding_status = onboarding_status.clone();
            let share_wayfarer_entry = share_wayfarer_entry.clone();
            let share_qr_image = share_qr_image.clone();
            let share_status_label = share_status_label.clone();
            dialog.connect_response(move |dialog, response| {
                if response == ResponseType::Accept {
                    if let Err(err) = delete_wayfarer_id() {
                        eprintln!("{err}");
                    }

                    match regenerate_local_identity() {
                        Ok(identity) => {
                            let key_preview: String =
                                identity.verifying_key_b64.chars().take(16).collect();
                            let device_preview: String = identity.device_id.chars().take(12).collect();
                            wayfarer_id_entry.set_text(&identity.wayfarer_id);
                            share_wayfarer_entry.set_text(&identity.wayfarer_id);
                            refresh_share_qr(
                                &identity.wayfarer_id,
                                &share_qr_image,
                                &share_status_label,
                            );
                            identity_meta_label.set_text(&format!(
                                "Identity metadata: device={} · device_id={}… · verify_key={}…",
                                identity.device_name, device_preview, key_preview
                            ));
                            onboarding_status.set_text(
                                "Step 2/2 · Identity reset complete. Share your new Wayfarer ID with contacts.",
                            );
                        }
                        Err(err) => {
                            eprintln!("{err}");
                            wayfarer_id_entry.set_text("");
                            share_wayfarer_entry.set_text("");
                            share_qr_image.set_from_file(Option::<&str>::None);
                            share_status_label.set_text("QR unavailable: identity missing");
                            identity_meta_label.set_text("Identity metadata: unavailable");
                        }
                    }
                }
                dialog.close();
            });

            dialog.present();
        });
    }

    {
        let views = views.clone();
        proceed_button.connect_clicked(move |_| {
            views.set_visible_child_name("settings");
        });
    }

    {
        let contact_id_entry = contact_id_entry.clone();
        let contact_alias_entry = contact_alias_entry.clone();
        let contacts_manage_list = contacts_manage_list.clone();
        views.connect_visible_child_name_notify(move |stack| {
            if stack.visible_child_name().as_deref() == Some("contacts") {
                contact_id_entry.set_text("");
                contact_alias_entry.set_text("");
                contacts_manage_list.select_row(Option::<&ListBoxRow>::None);
            }
        });
    }

    {
        let relay_secondary_label = relay_secondary_label.clone();
        open_logs_button.connect_clicked(move |_| match open_log_folder() {
            Ok(_) => {
                relay_secondary_label.set_text("Secondary relay status: opened logs folder");
            }
            Err(err) => {
                relay_secondary_label.set_text(&format!(
                    "Secondary relay status: failed to open log folder: {err}"
                ));
            }
        });
    }

    {
        let chat_status_label = chat_status_label.clone();
        open_chat_logs_button.connect_clicked(move |_| match open_log_folder() {
            Ok(_) => {
                chat_status_label.set_text("Opened log folder");
            }
            Err(err) => {
                chat_status_label.set_text(&format!("Failed to open log folder: {err}"));
            }
        });
    }

    {
        let tx = tx.clone();
        let wayfarer_id_entry = wayfarer_id_entry.clone();
        let identity_meta_label = identity_meta_label.clone();
        let onboarding_status = onboarding_status.clone();
        let share_wayfarer_entry = share_wayfarer_entry.clone();
        let share_qr_image = share_qr_image.clone();
        let share_status_label = share_status_label.clone();
        let relay_http_primary_entry = relay_http_primary_entry.clone();
        let relay_http_secondary_entry = relay_http_secondary_entry.clone();
        connect_button.connect_clicked(move |button| {
            button.set_sensitive(false);

            let identity = match ensure_local_identity() {
                Ok(identity) => identity,
                Err(err) => {
                    eprintln!("{err}");
                    button.set_sensitive(true);
                    return;
                }
            };

            let key_preview: String = identity.verifying_key_b64.chars().take(16).collect();
            let device_preview: String = identity.device_id.chars().take(12).collect();
            wayfarer_id_entry.set_text(&identity.wayfarer_id);
            share_wayfarer_entry.set_text(&identity.wayfarer_id);
            refresh_share_qr(&identity.wayfarer_id, &share_qr_image, &share_status_label);
            identity_meta_label.set_text(&format!(
                "Identity metadata: device={} · device_id={}… · verify_key={}…",
                identity.device_name, device_preview, key_preview
            ));
            onboarding_status
                .set_text("Step 2/2 · Identity provisioned automatically before diagnostics.");

            let relay_http_primary = normalize_http_endpoint(&relay_http_primary_entry.text());
            let relay_http_secondary = normalize_http_endpoint(&relay_http_secondary_entry.text());

            spawn_relay_checks(
                vec![relay_http_primary, relay_http_secondary],
                &identity.wayfarer_id,
                &identity.device_id,
                tx.clone(),
            );
        });
    }

    let (session_tx, session_rx) = channel::<SessionStatus>();

    {
        let session_tx = session_tx.clone();
        let relay_http_primary_entry = relay_http_primary_entry.clone();
        let recipient_entry = recipient_entry.clone();
        let body_entry = body_entry.clone();
        let chat_state = Rc::clone(&chat_state);
        send_button.connect_clicked(move |button| {
            button.set_sensitive(false);

            let identity = match ensure_local_identity() {
                Ok(identity) => identity,
                Err(err) => {
                    let _ = session_tx.send(SessionStatus {
                        op: SessionOp::Send,
                        text: format!("send failed: {err}"),
                        ack_msg_id: None,
                        outgoing_contact: None,
                        outgoing_text: None,
                        pulled_messages: Vec::new(),
                    });
                    return;
                }
            };

            let relay_ws =
                to_ws_endpoint(&normalize_http_endpoint(&relay_http_primary_entry.text()));
            let to = match chat_state.borrow().selected_contact.clone() {
                Some(contact) => contact,
                None => {
                    let _ = session_tx.send(SessionStatus {
                        op: SessionOp::Send,
                        text: "send failed: select a contact in Contacts first".to_string(),
                        ack_msg_id: None,
                        outgoing_contact: None,
                        outgoing_text: None,
                        pulled_messages: Vec::new(),
                    });
                    return;
                }
            };
            if !chat_state.borrow().contact_aliases.contains_key(&to) {
                let _ = session_tx.send(SessionStatus {
                    op: SessionOp::Send,
                    text: "send failed: selected contact is no longer in Contacts".to_string(),
                    ack_msg_id: None,
                    outgoing_contact: None,
                    outgoing_text: None,
                    pulled_messages: Vec::new(),
                });
                return;
            }
            recipient_entry.set_text(&to);
            let outgoing_text = body_entry.text().to_string();
            let payload_b64 = match build_envelope_payload_b64_from_utf8(&to, &outgoing_text) {
                Ok(payload) => payload,
                Err(err) => {
                    let _ = session_tx.send(SessionStatus {
                        op: SessionOp::Send,
                        text: format!("send failed: payload compose failed: {err}"),
                        ack_msg_id: None,
                        outgoing_contact: None,
                        outgoing_text: None,
                        pulled_messages: Vec::new(),
                    });
                    return;
                }
            };
            body_entry.set_text("");

            let auth = std::env::var("AETHOS_RELAY_AUTH_TOKEN").ok();
            let session_tx = session_tx.clone();
            thread::spawn(move || {
                let result = send_to_relay_v1_with_auth(
                    &relay_ws,
                    &identity.wayfarer_id,
                    &identity.device_id,
                    &to,
                    &payload_b64,
                    None,
                    Some(3600),
                    auth.as_deref(),
                );

                let status = match result {
                    Ok((msg_id, received_at, expires_at)) => SessionStatus {
                        op: SessionOp::Send,
                        text: format!(
                            "send_ok msg_id={} received_at={:?} expires_at={:?}",
                            msg_id, received_at, expires_at
                        ),
                        ack_msg_id: Some(msg_id),
                        outgoing_contact: Some(to),
                        outgoing_text: Some(outgoing_text),
                        pulled_messages: Vec::new(),
                    },
                    Err(err) => SessionStatus {
                        op: SessionOp::Send,
                        text: format!("send failed: {err}"),
                        ack_msg_id: None,
                        outgoing_contact: None,
                        outgoing_text: None,
                        pulled_messages: Vec::new(),
                    },
                };
                let _ = session_tx.send(status);
            });
        });
    }

    {
        let send_button = send_button.clone();
        body_entry.connect_activate(move |_| {
            send_button.emit_clicked();
        });
    }

    attach_status_poller(
        rx,
        connect_button,
        relay_primary_label,
        relay_secondary_label,
        diagnostics_text,
        relay_dot,
        relay_chip_text,
        relay_chip,
    );

    attach_session_poller(
        session_rx,
        send_button,
        chat_status_label,
        Rc::clone(&chat_state),
        contacts_list,
        Rc::clone(&contact_order),
        contact_id_entry.clone(),
        contact_alias_entry.clone(),
        messages_list,
        messages_scroll,
        thread_title,
        thread_contact_id_label,
        compact_contact_picker.clone(),
        Rc::clone(&picker_syncing),
    );

    attach_compact_adaptive_mode(
        window.clone(),
        chat_shell,
        contacts_revealer,
        compact_picker_revealer,
    );

    window.present();
}

fn spawn_relay_checks(
    relay_http_endpoints: Vec<String>,
    wayfarer_id: &str,
    device_id: &str,
    tx: Sender<RelayStatus>,
) {
    let wayfarer_id = wayfarer_id.to_string();
    let device_id = device_id.to_string();

    thread::spawn(move || {
        let mut session_manager =
            RelaySessionManager::new(relay_http_endpoints, RelaySessionConfig::default());
        let mut dispatcher = RelayRequestDispatcher::default();

        let shared_auth = std::env::var("AETHOS_RELAY_AUTH_TOKEN").ok();
        let relay_count = session_manager.relays().len();
        for relay_slot in 0..relay_count {
            session_manager.set_auth_token(relay_slot, shared_auth.clone());
        }

        let mut completed = 0;
        while completed < relay_count {
            let Some(selection) = session_manager.select_relay(Instant::now()) else {
                thread::sleep(Duration::from_millis(50));
                continue;
            };

            let outbound = dispatcher.register_outbound(
                "hello",
                json!({
                    "wayfarer_id": wayfarer_id,
                    "device_id": device_id,
                    "relay_slot": selection.relay_slot
                }),
            );

            let state = match selection.auth_token.as_deref() {
                Some(token) => connect_to_relay_with_auth(
                    &selection.relay_ws,
                    &wayfarer_id,
                    &device_id,
                    Some(token),
                ),
                None => connect_to_relay(&selection.relay_ws, &wayfarer_id, &device_id),
            };

            if state.starts_with("connected + hello_ok") {
                session_manager.mark_success(selection.relay_slot);
            } else {
                session_manager.mark_failure(selection.relay_slot);
            }

            let response = RelayFrame {
                correlation_id: outbound.correlation_id,
                message_type: if state.starts_with("connected + hello_ok") {
                    "hello_ack".to_string()
                } else {
                    "hello_error".to_string()
                },
                payload: json!({"relay_ws": selection.relay_ws, "state": state}),
            };

            let dispatch = match dispatcher.resolve_response(response) {
                Ok(resolved) => {
                    format!(
                        "corr={} req={} resp={} pending={} payload={}",
                        resolved.correlation_id,
                        resolved.request_message_type,
                        resolved.response_message_type,
                        dispatcher.pending_count(),
                        resolved.payload
                    )
                }
                Err(_) => "dispatcher error: unknown correlation".to_string(),
            };

            let _ = tx.send(RelayStatus {
                relay_slot: selection.relay_slot,
                relay_http: selection.relay_http,
                relay_ws: selection.relay_ws,
                state,
                dispatch,
            });
            completed += 1;
        }
    });
}

fn attach_status_poller(
    rx: Receiver<RelayStatus>,
    connect_button: Button,
    relay_primary_label: Label,
    relay_secondary_label: Label,
    diagnostics_text: TextView,
    relay_dot: Label,
    relay_chip_text: Label,
    relay_chip: GtkBox,
) {
    let mut completed = 0;

    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        while let Ok(status) = rx.try_recv() {
            completed += 1;
            let text = format!(
                "{} -> {} · {} · {}",
                status.relay_http, status.relay_ws, status.state, status.dispatch
            );
            append_local_log(&format!("relay_status: {text}"));

            if status.relay_slot == 0 {
                relay_primary_label.set_text(&format!("Primary relay status: {text}"));
            } else {
                relay_secondary_label.set_text(&format!("Secondary relay status: {text}"));
            }

            let buffer = diagnostics_text.buffer();
            let previous = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            let next = format!("{previous}\n- {text}");
            buffer.set_text(&next);
        }

        if completed >= 2 {
            let primary = relay_primary_label.text().to_string();
            let secondary = relay_secondary_label.text().to_string();
            update_relay_chip(
                &primary,
                &secondary,
                &relay_dot,
                &relay_chip_text,
                &relay_chip,
            );
            if let Err(err) = save_relay_session_cache(&RelaySessionCache {
                primary_status: primary,
                secondary_status: secondary,
            }) {
                eprintln!("{err}");
            }

            completed = 0;
            connect_button.set_sensitive(true);
        }

        glib::ControlFlow::Continue
    });
}

fn update_relay_chip(
    primary_status: &str,
    secondary_status: &str,
    relay_dot: &Label,
    relay_chip_text: &Label,
    relay_chip: &GtkBox,
) {
    relay_dot.remove_css_class("relay-dot-idle");
    relay_dot.remove_css_class("relay-dot-ok");
    relay_dot.remove_css_class("relay-dot-warn");
    relay_dot.remove_css_class("relay-dot-down");

    let primary_ok = primary_status.contains("connected + hello_ok");
    let secondary_ok = secondary_status.contains("connected + hello_ok");

    match (primary_ok, secondary_ok) {
        (true, true) => {
            relay_dot.add_css_class("relay-dot-ok");
            relay_chip_text.set_text("Relays: healthy (2/2)");
        }
        (true, false) | (false, true) => {
            relay_dot.add_css_class("relay-dot-warn");
            relay_chip_text.set_text("Relays: degraded (1/2)");
        }
        (false, false) => {
            let has_any_result = primary_status != "idle" || secondary_status != "idle";
            if has_any_result {
                relay_dot.add_css_class("relay-dot-down");
                relay_chip_text.set_text("Relays: unavailable (0/2)");
            } else {
                relay_dot.add_css_class("relay-dot-idle");
                relay_chip_text.set_text("Relays: idle");
            }
        }
    }

    relay_chip.set_tooltip_text(Some(&format!(
        "Primary: {}\nSecondary: {}",
        primary_status, secondary_status
    )));
}

fn attach_session_poller(
    rx: Receiver<SessionStatus>,
    send_button: Button,
    chat_status_label: Label,
    chat_state: Rc<RefCell<ChatState>>,
    contacts_list: ListBox,
    contact_order: Rc<RefCell<Vec<String>>>,
    contact_id_entry: Entry,
    contact_alias_entry: Entry,
    messages_list: ListBox,
    messages_scroll: ScrolledWindow,
    thread_title: Label,
    thread_contact_id_label: Label,
    compact_contact_picker: ComboBoxText,
    picker_syncing: Rc<Cell<bool>>,
) {
    glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
        while let Ok(status) = rx.try_recv() {
            match status.op {
                SessionOp::Send => send_button.set_sensitive(true),
            }

            chat_status_label.set_text(&status.text);
            append_local_log(&format!("session_status: {}", status.text));

            {
                let mut state = chat_state.borrow_mut();

                if let (Some(contact), Some(text)) = (
                    status.outgoing_contact.as_ref(),
                    status.outgoing_text.as_ref(),
                ) {
                    let outbound_msg_id = status
                        .ack_msg_id
                        .clone()
                        .unwrap_or_else(|| "outbound".to_string());
                    state
                        .threads
                        .entry(contact.clone())
                        .or_default()
                        .push(ChatMessage {
                            msg_id: outbound_msg_id,
                            text: text.clone(),
                            timestamp: format_timestamp_from_unix(now_unix_secs()),
                            direction: ChatDirection::Outgoing,
                        });
                    state.selected_contact = Some(contact.clone());
                }

                for pulled in &status.pulled_messages {
                    state
                        .threads
                        .entry(pulled.from_wayfarer_id.clone())
                        .or_default()
                        .push(ChatMessage {
                            msg_id: pulled.msg_id.clone(),
                            text: pulled.text.clone(),
                            timestamp: format_timestamp_from_unix(pulled.received_at),
                            direction: ChatDirection::Incoming,
                        });

                    if state.selected_contact.is_none() {
                        state.selected_contact = Some(pulled.from_wayfarer_id.clone());
                    }
                }
            }

            if let Err(err) = save_persisted_chat_state(&chat_state.borrow()) {
                append_local_log(&format!("persist_chat_state_failed: {err}"));
            }

            render_contacts(&chat_state.borrow(), &contacts_list, &contact_order);
            picker_syncing.set(true);
            sync_contact_picker(
                &chat_state.borrow(),
                &compact_contact_picker,
                &contact_order,
            );
            picker_syncing.set(false);
            sync_contact_form(
                &chat_state.borrow(),
                &contact_id_entry,
                &contact_alias_entry,
            );
            render_messages(
                &chat_state.borrow(),
                &messages_list,
                &messages_scroll,
                &thread_title,
                &thread_contact_id_label,
            );

            if status.outgoing_contact.is_some() {
                pulse_widget(&send_button, "pulse-send");
            }
            if !status.pulled_messages.is_empty() {
                pulse_widget(&messages_list, "pulse-receive");
            }
        }

        glib::ControlFlow::Continue
    });
}

fn render_contacts(
    state: &ChatState,
    contacts_list: &ListBox,
    contact_order: &Rc<RefCell<Vec<String>>>,
) {
    clear_listbox(contacts_list);

    let contacts = state.contact_aliases.keys().cloned().collect::<Vec<_>>();
    *contact_order.borrow_mut() = contacts.clone();

    for contact in contacts {
        let row = ListBoxRow::new();
        row.add_css_class("contact-row");

        let row_box = GtkBox::new(Orientation::Horizontal, 10);
        let avatar = Label::new(Some(&avatar_glyph(&contact)));
        avatar.add_css_class("contact-avatar");
        avatar.set_size_request(36, 36);

        let label_column = GtkBox::new(Orientation::Vertical, 1);
        let title = Label::new(Some(&contact_display_name(state, &contact)));
        title.set_xalign(0.0);
        title.add_css_class("contact-title");

        let subtitle = Label::new(Some(&format!("{}", tiny_wayfarer(&contact))));
        subtitle.set_xalign(0.0);
        subtitle.add_css_class("contact-subtitle");

        label_column.append(&title);
        label_column.append(&subtitle);

        let unread_count = state
            .threads
            .get(&contact)
            .map(|messages| {
                messages
                    .iter()
                    .filter(|msg| matches!(msg.direction, ChatDirection::Incoming))
                    .count()
            })
            .unwrap_or(0);

        if unread_count > 0 && state.selected_contact.as_deref() != Some(contact.as_str()) {
            let badge = Label::new(Some(&unread_count.to_string()));
            badge.add_css_class("unread-badge");
            row_box.append(&avatar);
            row_box.append(&label_column);
            row_box.append(&badge);
        } else {
            row_box.append(&avatar);
            row_box.append(&label_column);
        }

        row.set_child(Some(&row_box));
        contacts_list.append(&row);
    }
}

fn render_contacts_manager(
    state: &ChatState,
    contacts_list: &ListBox,
    contact_order: &Rc<RefCell<Vec<String>>>,
) {
    clear_listbox(contacts_list);

    let contacts = state.contact_aliases.keys().cloned().collect::<Vec<_>>();
    *contact_order.borrow_mut() = contacts.clone();

    for contact in contacts {
        let row = ListBoxRow::new();
        row.add_css_class("contact-row");

        let label = Label::new(Some(&format!(
            "{} · {}",
            contact_display_name(state, &contact),
            tiny_wayfarer(&contact)
        )));
        label.set_xalign(0.0);
        row.set_child(Some(&label));
        contacts_list.append(&row);
    }
}

fn sync_contact_picker(
    state: &ChatState,
    picker: &ComboBoxText,
    contact_order: &Rc<RefCell<Vec<String>>>,
) {
    let contacts = contact_order.borrow();
    picker.remove_all();
    for contact in contacts.iter() {
        picker.append(Some(contact), &contact_display_name(state, contact));
    }

    if let Some(selected) = state.selected_contact.as_ref() {
        picker.set_active_id(Some(selected));
    } else if !contacts.is_empty() {
        picker.set_active(Some(0));
    }
}

fn sync_contact_form(state: &ChatState, contact_id_entry: &Entry, contact_alias_entry: &Entry) {
    let Some(selected_contact) = state.selected_contact.as_ref() else {
        contact_id_entry.set_text("");
        contact_alias_entry.set_text("");
        return;
    };

    contact_id_entry.set_text(selected_contact);
    if let Some(alias) = state.contact_aliases.get(selected_contact) {
        contact_alias_entry.set_text(alias);
    } else {
        contact_alias_entry.set_text("");
    }
}

fn attach_compact_adaptive_mode(
    window: ApplicationWindow,
    chat_shell: Paned,
    contacts_revealer: Revealer,
    compact_picker_revealer: Revealer,
) {
    let last_compact = Cell::new(None::<bool>);
    glib::timeout_add_local(Duration::from_millis(180), move || {
        let width = window.width();
        let compact_mode = width > 0 && width < 900;

        if last_compact.get() != Some(compact_mode) {
            contacts_revealer.set_reveal_child(true);
            compact_picker_revealer.set_reveal_child(compact_mode);
            if compact_mode {
                let suggested = (width / 3).clamp(170, 260);
                chat_shell.set_position(suggested);
            } else {
                chat_shell.set_position(300);
            }
            last_compact.set(Some(compact_mode));
        }

        glib::ControlFlow::Continue
    });
}

fn render_messages(
    state: &ChatState,
    messages_list: &ListBox,
    messages_scroll: &ScrolledWindow,
    thread_title: &Label,
    thread_contact_id_label: &Label,
) {
    clear_listbox(messages_list);

    let Some(selected_contact) = state.selected_contact.as_ref() else {
        thread_title.set_text("Thread");
        thread_contact_id_label.set_text("");
        return;
    };

    thread_title.set_text(&format!(
        "Thread · {}",
        contact_display_name(state, selected_contact)
    ));
    if state.show_full_contact_id {
        thread_contact_id_label.set_text(selected_contact);
    } else {
        thread_contact_id_label.set_text(&tiny_wayfarer(selected_contact));
    }

    if let Some(messages) = state.threads.get(selected_contact) {
        for message in messages {
            let row = ListBoxRow::new();
            row.set_selectable(false);

            let bubble_wrap = GtkBox::new(Orientation::Horizontal, 8);
            let bubble = Label::new(Some(&message.text));
            bubble.set_wrap(true);
            bubble.set_xalign(0.0);
            bubble.add_css_class("chat-bubble");

            let metadata = Label::new(Some(&format!("id={}", message.msg_id)));
            metadata.set_text(&format!(
                "{} · {}",
                message.timestamp,
                short_msg_id(&message.msg_id)
            ));
            metadata.add_css_class("bubble-meta");
            metadata.set_xalign(0.0);

            let bubble_column = GtkBox::new(Orientation::Vertical, 2);
            bubble_column.append(&bubble);
            bubble_column.append(&metadata);

            match message.direction {
                ChatDirection::Outgoing => {
                    bubble.add_css_class("chat-bubble-outgoing");
                    bubble_wrap.set_halign(gtk4::Align::End);
                }
                ChatDirection::Incoming => {
                    bubble.add_css_class("chat-bubble-incoming");
                    bubble_wrap.set_halign(gtk4::Align::Start);
                }
            }

            bubble_wrap.append(&bubble_column);
            row.set_child(Some(&bubble_wrap));
            messages_list.append(&row);
        }
    }

    scroll_thread_to_bottom(messages_scroll.clone());
}

fn clear_listbox(list: &ListBox) {
    while let Some(child) = list.first_child() {
        if let Ok(row) = child.downcast::<ListBoxRow>() {
            list.remove(&row);
        } else {
            break;
        }
    }
}

fn scroll_thread_to_bottom(messages_scroll: ScrolledWindow) {
    glib::timeout_add_local(Duration::from_millis(20), move || {
        let adj = messages_scroll.vadjustment();
        let bottom = (adj.upper() - adj.page_size()).max(adj.lower());
        adj.set_value(bottom);
        glib::ControlFlow::Break
    });
}

fn short_wayfarer(value: &str) -> String {
    if value.len() <= 14 {
        return value.to_string();
    }
    format!("{}…{}", &value[0..8], &value[value.len() - 6..])
}

fn contact_display_name(state: &ChatState, wayfarer_id: &str) -> String {
    state
        .contact_aliases
        .get(wayfarer_id)
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| short_wayfarer(wayfarer_id))
}

fn tiny_wayfarer(value: &str) -> String {
    if value.len() <= 24 {
        return value.to_string();
    }
    format!("{}…{}", &value[0..12], &value[value.len() - 8..])
}

fn avatar_glyph(wayfarer_id: &str) -> String {
    let tail = wayfarer_id.chars().last().unwrap_or('0');
    match tail {
        '0' | '1' | '2' => "◉".to_string(),
        '3' | '4' | '5' => "◆".to_string(),
        '6' | '7' | '8' => "▲".to_string(),
        _ => "●".to_string(),
    }
}

fn short_msg_id(value: &str) -> String {
    if value.len() <= 14 {
        return value.to_string();
    }
    format!("{}…{}", &value[0..6], &value[value.len() - 6..])
}

fn format_timestamp_from_unix(unix_secs: i64) -> String {
    if let Ok(dt) = glib::DateTime::from_unix_local(unix_secs) {
        return dt
            .format("%I:%M %p")
            .map(|s| s.to_string())
            .unwrap_or_else(|_| "--:--".to_string());
    }
    "--:--".to_string()
}

fn now_unix_secs() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs() as i64,
        Err(_) => 0,
    }
}

fn pulse_widget<W>(widget: &W, class_name: &'static str)
where
    W: IsA<gtk4::Widget> + Clone + 'static,
{
    let widget = widget.clone().upcast::<gtk4::Widget>();
    widget.add_css_class(class_name);
    glib::timeout_add_local(Duration::from_millis(240), move || {
        widget.remove_css_class(class_name);
        glib::ControlFlow::Break
    });
}

fn ensure_linux_desktop_integration() -> Result<(), String> {
    #[cfg(not(target_os = "linux"))]
    {
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME")
            .map_err(|_| "HOME not set for desktop integration".to_string())?;

        let applications_dir = Path::new(&home).join(".local/share/applications");
        let icon_dir = Path::new(&home).join(".local/share/icons/hicolor/256x256/apps");

        fs::create_dir_all(&applications_dir)
            .map_err(|err| format!("failed creating applications dir: {err}"))?;
        fs::create_dir_all(&icon_dir).map_err(|err| format!("failed creating icon dir: {err}"))?;

        let icon_target = icon_dir.join(format!("{}.png", APP_ID));
        let icon_source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/img/logo.png");
        if icon_source.exists() {
            fs::copy(&icon_source, &icon_target)
                .map_err(|err| format!("failed to copy app icon into icon theme path: {err}"))?;
        }

        let desktop_path = applications_dir.join(format!("{}.desktop", APP_ID));
        let exec = std::env::current_exe()
            .map_err(|err| format!("failed to determine executable path: {err}"))?;

        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Aethos Linux\nExec={}\nIcon={}\nTerminal=false\nCategories=Network;Chat;\nStartupNotify=true\nStartupWMClass={}\n",
            shell_escape(exec.as_os_str().to_string_lossy().as_ref()),
            APP_ID,
            APP_ID,
        );

        fs::write(&desktop_path, desktop)
            .map_err(|err| format!("failed to write desktop entry: {err}"))?;
    }

    Ok(())
}

fn shell_escape(value: &str) -> String {
    if !value.contains([' ', '\'', '"']) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn append_local_log(message: &str) {
    if let Err(err) = append_local_log_inner(message) {
        eprintln!("local log warning: {err}");
    }
}

fn append_local_log_inner(message: &str) -> Result<(), String> {
    let path = app_log_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed creating app log directory: {err}"))?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| format!("failed opening app log file at {}: {err}", path.display()))?;

    let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_secs(),
        Err(_) => 0,
    };

    writeln!(file, "[{now}] {message}")
        .map_err(|err| format!("failed writing app log file at {}: {err}", path.display()))
}

fn app_log_file_path() -> PathBuf {
    if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME") {
        if !xdg_state_home.trim().is_empty() {
            return Path::new(&xdg_state_home)
                .join("aethos-linux")
                .join(APP_LOG_FILE_NAME);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home)
            .join(".local")
            .join("state")
            .join("aethos-linux")
            .join(APP_LOG_FILE_NAME);
    }

    std::env::temp_dir().join(APP_LOG_FILE_NAME)
}

fn open_log_folder() -> Result<(), String> {
    let log_dir = app_log_file_path()
        .parent()
        .map(|path| path.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    Command::new("xdg-open")
        .arg(&log_dir)
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("{err}"))
}

fn load_persisted_chat_state() -> Result<Option<PersistedChatState>, String> {
    let path = chat_history_file_path();
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path).map_err(|err| {
        format!(
            "failed to read chat history file at {}: {err}",
            path.display()
        )
    })?;
    let data: PersistedChatState = serde_json::from_str(&content).map_err(|err| {
        format!(
            "failed to parse chat history file at {}: {err}",
            path.display()
        )
    })?;
    Ok(Some(data))
}

fn save_persisted_chat_state(state: &ChatState) -> Result<(), String> {
    let path = chat_history_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("failed creating chat history directory: {err}"))?;
    }

    let payload = PersistedChatState {
        selected_contact: state.selected_contact.clone(),
        threads: state.threads.clone(),
    };
    let serialized = serde_json::to_string_pretty(&payload)
        .map_err(|err| format!("failed to serialize chat history: {err}"))?;
    fs::write(&path, serialized).map_err(|err| {
        format!(
            "failed to write chat history file at {}: {err}",
            path.display()
        )
    })
}

fn chat_history_file_path() -> PathBuf {
    if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME") {
        if !xdg_state_home.trim().is_empty() {
            return Path::new(&xdg_state_home)
                .join("aethos-linux")
                .join(CHAT_HISTORY_FILE_NAME);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home)
            .join(".local")
            .join("state")
            .join("aethos-linux")
            .join(CHAT_HISTORY_FILE_NAME);
    }

    std::env::temp_dir().join(CHAT_HISTORY_FILE_NAME)
}

fn share_qr_file_path() -> PathBuf {
    if let Ok(xdg_state_home) = std::env::var("XDG_STATE_HOME") {
        if !xdg_state_home.trim().is_empty() {
            return Path::new(&xdg_state_home)
                .join("aethos-linux")
                .join(SHARE_QR_FILE_NAME);
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        return Path::new(&home)
            .join(".local")
            .join("state")
            .join("aethos-linux")
            .join(SHARE_QR_FILE_NAME);
    }

    std::env::temp_dir().join(SHARE_QR_FILE_NAME)
}

fn refresh_share_qr(wayfarer_id: &str, share_qr_image: &Image, share_status_label: &Label) {
    match generate_share_qr_png(wayfarer_id) {
        Ok(path) => {
            share_qr_image.set_from_file(path.to_str());
            share_status_label.set_text("QR ready to share");
        }
        Err(err) => {
            share_qr_image.set_from_file(Option::<&str>::None);
            share_status_label.set_text(&format!("QR generation failed: {err}"));
        }
    }
}

fn generate_share_qr_png(wayfarer_id: &str) -> Result<PathBuf, String> {
    let code = QrCode::new(wayfarer_id.as_bytes())
        .map_err(|err| format!("failed generating QR payload: {err}"))?;
    let scale: u32 = 8;
    let border: u32 = 4;
    let luma: ImageBuffer<Luma<u8>, Vec<u8>> = code
        .render::<Luma<u8>>()
        .quiet_zone(false)
        .module_dimensions(scale, scale)
        .build();

    let inner_w = luma.width();
    let inner_h = luma.height();
    let width = inner_w + border * scale * 2;
    let height = inner_h + border * scale * 2;
    let mut rgba = RgbaImage::from_pixel(width, height, Rgba([255, 255, 255, 255]));

    for y in 0..inner_h {
        for x in 0..inner_w {
            let px = luma.get_pixel(x, y).0[0];
            let color = if px < 128 {
                Rgba([16, 18, 28, 255])
            } else {
                Rgba([255, 255, 255, 255])
            };
            rgba.put_pixel(x + border * scale, y + border * scale, color);
        }
    }

    let monogram = a_monogram_icon_rgba((width / 6).max(36));
    overlay_center(&mut rgba, &monogram);

    let path = share_qr_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("failed creating share qr dir: {err}"))?;
    }
    rgba.save(&path)
        .map_err(|err| format!("failed saving share QR image: {err}"))?;
    Ok(path)
}

fn overlay_center(base: &mut RgbaImage, overlay: &RgbaImage) {
    let offset_x = base.width().saturating_sub(overlay.width()) / 2;
    let offset_y = base.height().saturating_sub(overlay.height()) / 2;

    for y in 0..overlay.height() {
        for x in 0..overlay.width() {
            let src = overlay.get_pixel(x, y);
            let alpha = src[3] as f32 / 255.0;
            if alpha <= 0.0 {
                continue;
            }

            let dst = base.get_pixel_mut(x + offset_x, y + offset_y);
            for i in 0..3 {
                let blended = (src[i] as f32 * alpha) + (dst[i] as f32 * (1.0 - alpha));
                dst[i] = blended.round() as u8;
            }
            dst[3] = 255;
        }
    }
}

fn a_monogram_icon_rgba(size: u32) -> RgbaImage {
    let mut img = RgbaImage::from_pixel(size, size, Rgba([255, 255, 255, 0]));
    let cx = size as f32 * 0.5;
    let cy = size as f32 * 0.5;
    let radius = size as f32 * 0.44;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - cx;
            let dy = y as f32 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= radius {
                img.put_pixel(x, y, Rgba([250, 252, 255, 245]));
            }
        }
    }

    let stroke = Rgba([33, 79, 188, 255]);
    let left_x = (size as f32 * 0.32) as i32;
    let right_x = (size as f32 * 0.68) as i32;
    let top_y = (size as f32 * 0.28) as i32;
    let bottom_y = (size as f32 * 0.74) as i32;
    let cross_y = (size as f32 * 0.54) as i32;

    draw_line(
        &mut img,
        left_x,
        bottom_y,
        (size as f32 * 0.5) as i32,
        top_y,
        stroke,
    );
    draw_line(
        &mut img,
        right_x,
        bottom_y,
        (size as f32 * 0.5) as i32,
        top_y,
        stroke,
    );
    draw_line(
        &mut img,
        (size as f32 * 0.39) as i32,
        cross_y,
        (size as f32 * 0.61) as i32,
        cross_y,
        stroke,
    );

    img
}

fn draw_line(img: &mut RgbaImage, mut x0: i32, mut y0: i32, x1: i32, y1: i32, color: Rgba<u8>) {
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        for oy in -1..=1 {
            for ox in -1..=1 {
                let px = x0 + ox;
                let py = y0 + oy;
                if px >= 0 && py >= 0 && (px as u32) < img.width() && (py as u32) < img.height() {
                    img.put_pixel(px as u32, py as u32, color);
                }
            }
        }

        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x0 += sx;
        }
        if e2 <= dx {
            err += dx;
            y0 += sy;
        }
    }
}

fn apply_styles() {
    let provider = CssProvider::new();
    provider.load_from_data(
        "
        window {
            background: radial-gradient(circle at 70% -20%, rgba(51, 167, 255, 0.18), rgba(16, 27, 61, 0) 36%),
                        radial-gradient(circle at 15% 20%, rgba(137, 92, 255, 0.18), rgba(16, 27, 61, 0) 40%),
                        linear-gradient(180deg, #060814 0%, #050612 100%);
            color: #f1f3ff;
            font-family: \"SF Pro Display\", \"Inter\", \"Noto Sans\", sans-serif;
        }

        .root {
            background: transparent;
        }

        .thread-contact-id {
            font-size: 11px;
            color: rgba(149, 159, 191, 0.95);
        }

        .footer-bar {
            border-top: 1px solid rgba(107, 120, 172, 0.28);
            margin-top: 6px;
            padding-top: 8px;
        }

        .footer-note {
            font-size: 11px;
            color: rgba(139, 149, 181, 0.9);
        }

        .footer-logo {
            border-radius: 6px;
            opacity: 0.65;
        }

        .relay-chip {
            border-radius: 12px;
            padding: 4px 10px;
            border: 1px solid rgba(98, 111, 166, 0.36);
            background: rgba(19, 24, 45, 0.84);
        }

        .relay-dot {
            font-size: 12px;
            font-weight: 800;
        }

        .relay-dot-idle {
            color: rgba(140, 151, 185, 0.9);
        }

        .relay-dot-ok {
            color: rgba(79, 219, 130, 0.95);
        }

        .relay-dot-warn {
            color: rgba(255, 193, 74, 0.95);
        }

        .relay-dot-down {
            color: rgba(251, 97, 124, 0.95);
        }

        .relay-chip-text {
            font-size: 11px;
            color: rgba(198, 207, 239, 0.94);
        }

        .settings-shell {
            padding: 8px;
        }

        .settings-group-title {
            font-size: 13px;
            font-weight: 700;
            color: rgba(182, 193, 231, 0.95);
            letter-spacing: 0.02em;
            margin-top: 6px;
        }

        .settings-group-hint {
            font-size: 12px;
            color: rgba(150, 161, 197, 0.9);
            margin-bottom: 2px;
        }

        .settings-card {
            background: rgba(25, 29, 50, 0.78);
            border: 1px solid rgba(113, 128, 186, 0.3);
            border-radius: 14px;
            padding: 12px;
        }

        .settings-scroll {
            border-radius: 12px;
        }

        .section-title {
            font-size: 17px;
            font-weight: 700;
            color: #c2b2ff;
            letter-spacing: 0.04em;
        }

        .glass-panel {
            border-radius: 18px;
            padding: 14px;
            border: 1px solid rgba(140, 154, 216, 0.26);
            background: linear-gradient(180deg, rgba(31, 35, 56, 0.9), rgba(18, 21, 39, 0.9));
            box-shadow: 0 12px 26px rgba(4, 6, 20, 0.55);
        }

        entry, textview, list {
            border-radius: 10px;
            border: 1px solid rgba(114, 126, 180, 0.42);
            background: rgba(17, 21, 41, 0.82);
            color: #f1f3ff;
            padding: 8px;
        }

        entry placeholder {
            color: rgba(149, 156, 182, 0.75);
        }

        stackswitcher button {
            border-radius: 10px;
            margin-right: 6px;
            border: 1px solid rgba(103, 115, 171, 0.46);
            background: rgba(19, 23, 45, 0.84);
            color: #8f98b4;
            padding: 7px 12px;
        }

        stackswitcher button:checked {
            color: #e7ebff;
            background: linear-gradient(90deg, rgba(33, 119, 214, 0.65), rgba(65, 89, 213, 0.65));
            border-color: rgba(100, 171, 255, 0.7);
        }

        button.action {
            border-radius: 10px;
            border: 1px solid rgba(88, 165, 255, 0.58);
            background: linear-gradient(90deg, rgba(20, 117, 231, 0.88), rgba(49, 137, 247, 0.88));
            color: #f3f7ff;
            font-weight: 700;
            padding: 8px 12px;
        }

        button.compact {
            border-radius: 9px;
            border: 1px solid rgba(108, 126, 194, 0.42);
            background: rgba(23, 28, 53, 0.86);
            color: rgba(198, 208, 242, 0.96);
            padding: 5px 9px;
        }

        button.danger {
            border-radius: 10px;
            border: 1px solid rgba(251, 121, 150, 0.52);
            background: linear-gradient(90deg, rgba(145, 44, 74, 0.74), rgba(119, 35, 64, 0.74));
            color: #ffe7ef;
            font-weight: 700;
            padding: 8px 12px;
        }

        .warning {
            color: rgba(255, 194, 205, 0.95);
            font-size: 13px;
        }

        .chat-shell {
            min-height: 300px;
        }

        .contacts-pane {
            min-width: 230px;
        }

        .composer-bar {
            border-radius: 18px;
            padding: 7px;
            border: 1px solid rgba(95, 109, 169, 0.38);
            background: rgba(20, 24, 43, 0.86);
        }

        .message-entry {
            border-radius: 15px;
            min-height: 42px;
        }

        .recipient-entry {
            font-size: 12px;
        }

        .send-fab {
            border-radius: 19px;
            min-width: 38px;
            min-height: 38px;
            padding: 0;
            font-size: 18px;
        }

        .pulse-send {
            box-shadow: 0 0 0 5px rgba(56, 167, 255, 0.22);
            border-color: rgba(131, 208, 255, 0.92);
        }

        .pulse-receive {
            box-shadow: inset 0 0 0 1px rgba(153, 187, 255, 0.52);
            background: rgba(35, 43, 79, 0.42);
        }

        .contact-list row {
            border-radius: 12px;
            margin-bottom: 5px;
            background: rgba(21, 26, 49, 0.7);
            border: 1px solid rgba(101, 111, 162, 0.26);
            padding: 6px;
        }

        .contact-list row:selected {
            background: rgba(63, 74, 124, 0.82);
            border-color: rgba(113, 150, 236, 0.7);
        }

        .contact-avatar {
            border-radius: 18px;
            min-width: 36px;
            min-height: 36px;
            background: linear-gradient(180deg, rgba(34, 111, 182, 0.9), rgba(28, 74, 138, 0.9));
            color: #a995ff;
            font-size: 14px;
            font-weight: 700;
            padding: 7px;
        }

        .contact-title {
            font-size: 15px;
            font-weight: 700;
            color: #eef2ff;
        }

        .contact-subtitle {
            font-size: 11px;
            color: rgba(157, 166, 200, 0.9);
        }

        .unread-badge {
            border-radius: 10px;
            background: rgba(17, 145, 245, 0.9);
            color: #f6fbff;
            font-size: 11px;
            font-weight: 700;
            padding: 2px 7px;
        }

        .messages-list row {
            background: transparent;
            border: none;
            margin: 2px 0;
        }

        .chat-bubble {
            border-radius: 16px;
            padding: 8px 11px;
            line-height: 1.3;
            font-size: 14px;
        }

        .chat-bubble-incoming {
            background: rgba(54, 60, 84, 0.92);
            color: #eff2ff;
        }

        .chat-bubble-outgoing {
            background: rgba(67, 60, 115, 0.94);
            color: #eff2ff;
        }

        .bubble-meta {
            font-size: 11px;
            color: rgba(155, 164, 196, 0.8);
        }

        expander > title {
            color: rgba(162, 173, 213, 0.95);
            font-weight: 600;
        }

        expander > title > arrow {
            color: rgba(133, 147, 201, 0.95);
        }
        ",
    );

    if let Some(display) = Display::default() {
        gtk4::style_context_add_provider_for_display(
            &display,
            &provider,
            STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}
