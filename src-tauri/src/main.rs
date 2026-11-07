#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if matches!(
        args.first().map(|s| s.as_str()),
        Some("engine") | Some("new") | Some("import") | Some("export")
    ) {
        std::process::exit(lantern_lib::cli::run(&args));
    }
    lantern_lib::run()
}
