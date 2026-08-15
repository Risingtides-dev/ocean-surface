fn main() {
    let context = tauri::generate_context!("tauri.rooms-acceptance.conf.json");
    ocean_tauri::run_rooms_acceptance(context);
}
