// Bez okna konsoli w wersji release.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    pdf_workbench_lib::run()
}
