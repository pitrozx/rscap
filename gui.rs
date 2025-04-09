// src/gui.rs

use gtk::prelude::*;
use gtk::{
    Application, ApplicationWindow, Box, Button, ComboBoxText, Entry, FileChooserAction,
    FileChooserDialog, Label, Orientation, ResponseType, RadioButton, SpinButton,
};
use std::env::args;

#[derive(Debug, Clone)]
pub struct RecordParams {
    /// Для OCI здесь используется как имя bucket (или часть логики формирования пути)
    pub output_folder: String,
    /// Шаблон имени объекта (например, "recording_2025_04_09")
    pub filename_template: String,
    /// Контейнер: mp4 или mkv
    pub container: String,
    /// Битрейт в килобитах
    pub bitrate: u32,
    /// Режим кодирования: CBR или VBR
    pub encoding_mode: String,
    /// Устройство для захвата звука
    pub audio_device: String,
}

pub fn run_gui<F: Fn(RecordParams) + 'static>(callback: F) {
    let app = Application::new(
        Some("com.example.screenrecorder"),
        Default::default(),
    )
    .expect("Failed to initialize GTK application");

    app.connect_activate(move |app| {
        let window = ApplicationWindow::new(app);
        window.set_title("Screen Recorder");
        window.set_default_size(400, 300);

        let vbox = Box::new(Orientation::Vertical, 10);
        vbox.set_margin_top(10);
        vbox.set_margin_bottom(10);
        vbox.set_margin_start(10);
        vbox.set_margin_end(10);
        window.add(&vbox);

        // 1. Выбор «bucket» (выходной папки – OCI bucket)
        let folder_hbox = Box::new(Orientation::Horizontal, 5);
        let folder_label = Label::new(Some("Output Bucket:"));
        let folder_entry = Entry::new();
        folder_entry.set_editable(false);
        let folder_button = Button::with_label("Choose Bucket");
        folder_hbox.pack_start(&folder_label, false, false, 0);
        folder_hbox.pack_start(&folder_entry, true, true, 0);
        folder_hbox.pack_start(&folder_button, false, false, 0);
        vbox.pack_start(&folder_hbox, false, false, 0);

        // 2. Шаблон имени объекта
        let filename_hbox = Box::new(Orientation::Horizontal, 5);
        let filename_label = Label::new(Some("Filename Template:"));
        let filename_entry = Entry::new();
        filename_hbox.pack_start(&filename_label, false, false, 0);
        filename_hbox.pack_start(&filename_entry, true, true, 0);
        vbox.pack_start(&filename_hbox, false, false, 0);

        // 3. Выбор контейнера: mp4 или mkv
        let container_hbox = Box::new(Orientation::Horizontal, 5);
        let container_label = Label::new(Some("Container:"));
        let container_combo = ComboBoxText::new();
        container_combo.append_text("mp4");
        container_combo.append_text("mkv");
        container_combo.set_active(Some(0));
        container_hbox.pack_start(&container_label, false, false, 0);
        container_hbox.pack_start(&container_combo, false, false, 0);
        vbox.pack_start(&container_hbox, false, false, 0);

        // 4. Задание битрейта (в килобитах)
        let bitrate_hbox = Box::new(Orientation::Horizontal, 5);
        let bitrate_label = Label::new(Some("Bitrate (kbps):"));
        let bitrate_spin = SpinButton::new_with_range(100.0, 10000.0, 100.0);
        bitrate_spin.set_value(1000.0);
        bitrate_hbox.pack_start(&bitrate_label, false, false, 0);
        bitrate_hbox.pack_start(&bitrate_spin, false, false, 0);
        vbox.pack_start(&bitrate_hbox, false, false, 0);

        // 5. Режим кодирования: CBR или VBR
        let mode_hbox = Box::new(Orientation::Horizontal, 5);
        let mode_label = Label::new(Some("Encoding Mode:"));
        let cbr_radio = RadioButton::with_label(None, "CBR");
        let vbr_radio = RadioButton::with_label_from_widget(&cbr_radio, "VBR");
        mode_hbox.pack_start(&mode_label, false, false, 0);
        mode_hbox.pack_start(&cbr_radio, false, false, 0);
        mode_hbox.pack_start(&vbr_radio, false, false, 0);
        vbox.pack_start(&mode_hbox, false, false, 0);

        // 6. Устройство для захвата звука
        let audio_hbox = Box::new(Orientation::Horizontal, 5);
        let audio_label = Label::new(Some("Audio Device:"));
        let audio_combo = ComboBoxText::new();
        // Пример заполнения: в реальном приложении можно получить список устройств через API
        audio_combo.append_text("default");
        audio_combo.append_text("Device 1");
        audio_combo.append_text("Device 2");
        audio_combo.set_active(Some(0));
        audio_hbox.pack_start(&audio_label, false, false, 0);
        audio_hbox.pack_start(&audio_combo, false, false, 0);
        vbox.pack_start(&audio_hbox, false, false, 0);

        // Кнопка "Start Recording"
        let start_button = Button::with_label("Start Recording");
        vbox.pack_start(&start_button, false, false, 0);

        // Выбор «bucket» через диалог (FileChooserDialog в режиме выбора папки)
        let folder_entry_clone = folder_entry.clone();
        let win_clone = window.clone();
        folder_button.connect_clicked(move |_| {
            let dialog = FileChooserDialog::new(
                Some("Select Output Bucket (Folder)"),
                Some(&win_clone),
                FileChooserAction::SelectFolder,
            );
            dialog.add_button("Cancel", ResponseType::Cancel);
            dialog.add_button("Select", ResponseType::Accept);
            if dialog.run() == ResponseType::Accept {
                if let Some(folder) = dialog.get_filename() {
                    if let Some(folder_str) = folder.to_str() {
                        folder_entry_clone.set_text(folder_str);
                    }
                }
            }
            dialog.close();
        });

        // При клике по кнопке собираем параметры и вызываем callback
        start_button.connect_clicked(move |_| {
            let output_folder = folder_entry.get_text().to_string();
            let filename_template = filename_entry.get_text().to_string();
            let container = container_combo
                .get_active_text()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "mp4".to_string());
            let bitrate = bitrate_spin.get_value_as_int() as u32;
            let encoding_mode = if cbr_radio.get_active() {
                "CBR".to_string()
            } else {
                "VBR".to_string()
            };
            let audio_device = audio_combo
                .get_active_text()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "default".to_string());

            let params = RecordParams {
                output_folder,
                filename_template,
                container,
                bitrate,
                encoding_mode,
                audio_device,
            };
            callback(params);
        });

        window.show_all();
    });

    app.run(&args().collect::<Vec<_>>());
}
