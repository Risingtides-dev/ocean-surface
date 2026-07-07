// Thin binary — all wiring lives in the library so `tauri::mobile_entry_point`
// (a later phase) can reuse `ocean_tauri::run`.

fn main() {
    ocean_tauri::run();
}
