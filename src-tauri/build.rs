/// Wbudowany klient OAuth Google (logowanie do Gmaila jednym kliknięciem). Dane klienta nie są
/// w repozytorium: pochodzą ze zmiennych PDFWB_GOOGLE_CLIENT_ID / PDFWB_GOOGLE_CLIENT_SECRET albo
/// z pliku %USERPROFILE%\.tauri\pdf-workbench-google.json (JSON pobrany z Google Cloud).
fn google_client() {
    println!("cargo:rerun-if-env-changed=PDFWB_GOOGLE_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=PDFWB_GOOGLE_CLIENT_SECRET");
    let mut id = std::env::var("PDFWB_GOOGLE_CLIENT_ID").ok();
    let mut secret = std::env::var("PDFWB_GOOGLE_CLIENT_SECRET").ok();
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        let file = std::path::Path::new(&home).join(".tauri").join("pdf-workbench-google.json");
        println!("cargo:rerun-if-changed={}", file.display());
        if let Ok(text) = std::fs::read_to_string(&file) {
            let v: serde_json::Value = serde_json::from_str(&text).expect("pdf-workbench-google.json: niepoprawny JSON");
            let c = if v["installed"].is_object() { &v["installed"] } else { &v };
            id = id.or_else(|| c["client_id"].as_str().map(str::to_string));
            secret = secret.or_else(|| c["client_secret"].as_str().map(str::to_string));
        }
    }
    if let (Some(id), Some(secret)) = (id, secret) {
        println!("cargo:rustc-env=PDFWB_GOOGLE_CLIENT_ID={}", id.trim());
        println!("cargo:rustc-env=PDFWB_GOOGLE_CLIENT_SECRET={}", secret.trim());
    }
}

fn main() {
    google_client();
    tauri_build::build()
}
