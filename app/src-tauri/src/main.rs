#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod bridge;
mod persistence;
fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            bridge::load_layout,
            bridge::save_layout,
            bridge::close_editor,
            bridge::home_machine,
            bridge::connected_screens,
            bridge::portuguese
        ])
        .run(tauri::generate_context!())
        .expect("could not start the MouseX screen editor");
}
