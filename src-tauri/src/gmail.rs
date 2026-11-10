//! Poczta do podpisu przez Gmail API: logowanie OAuth aplikacji desktopowej (pętla zwrotna
//! 127.0.0.1 + PKCE), lista maili z PDF-ami, pobieranie załączników i szkic „Odpowiedz wszystkim”
//! z podpisanymi plikami w tym samym wątku. Szkic wysyła się dopiero na wyraźne „Wyślij” użytkownika.

use base64::engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD};
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

type Res<T> = Result<T, String>;

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REVOKE_URL: &str = "https://oauth2.googleapis.com/revoke";
const API: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const UPLOAD_API: &str = "https://gmail.googleapis.com/upload/gmail/v1/users/me";
const SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
pub const KEY_SECRET: &str = "google-client-secret";
pub const KEY_REFRESH: &str = "google-refresh-token";
pub const DEFAULT_QUERY: &str = "in:inbox has:attachment filename:pdf -label:podpisane newer_than:60d";
pub const DONE_LABEL: &str = "Podpisane";
const MAX_LIST: usize = 30;
/// Powyżej tej wielkości wiadomość idzie przez endpoint wysyłania plików zamiast pola `raw`.
const RAW_LIMIT: usize = 3_000_000;

// --- Logowanie ---------------------------------------------------------------------------------

fn random_b64(n: usize) -> Res<String> {
    use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
    let mut buf = vec![0u8; n];
    unsafe { BCryptGenRandom(None, &mut buf, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }.ok().map_err(|e| e.to_string())?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs.iter().map(|(k, v)| format!("{k}={}", urlencoding::encode(v))).collect::<Vec<_>>().join("&")
}

fn query_param(query: &str, name: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (k == name).then(|| urlencoding::decode(&v.replace('+', " ")).map(|s| s.into_owned()).unwrap_or_default())
    })
}

/// Otwiera adres w domyślnej przeglądarce (bez powłoki, więc „&” w adresie jest bezpieczne).
pub fn open_in_browser(url: &str) -> Res<()> {
    std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).spawn().map(|_| ()).map_err(|e| e.to_string())
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>PDF Workbench</title><body style=\"font-family:Segoe UI,sans-serif;padding:48px\"><h2>PDF Workbench połączono z Gmailem</h2><p>Możesz zamknąć tę kartę i wrócić do programu.</p>";

fn respond(stream: &mut std::net::TcpStream, status: &str, body: &str) {
    let _ = write!(stream, "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
}

pub struct Grant {
    pub code: String,
    pub verifier: String,
    pub redirect: String,
}

/// Otwiera stronę zgody Google i czeka (do 5 minut) na przekierowanie z kodem na 127.0.0.1.
pub fn authorize(client_id: &str) -> Res<Grant> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("Nie można otworzyć portu logowania: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let verifier = random_b64(48)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_b64(18)?;
    let url = format!(
        "{AUTH_URL}?{}",
        form(&[
            ("client_id", client_id),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
        ])
    );
    open_in_browser(&url)?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(300);
    loop {
        if Instant::now() > deadline {
            return Err("Nie dokończono logowania w przeglądarce w ciągu 5 minut.".into());
        }
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(150));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let mut buf = [0u8; 8192];
        let n = stream.read(&mut buf).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let target = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
        let query = match target.split_once('?') {
            Some(("/", q)) => q.to_string(),
            _ => {
                respond(&mut stream, "404 Not Found", "");
                continue;
            }
        };
        if query_param(&query, "state").as_deref() != Some(state.as_str()) {
            respond(&mut stream, "400 Bad Request", "Nieprawidłowe żądanie.");
            continue;
        }
        if let Some(e) = query_param(&query, "error") {
            respond(&mut stream, "200 OK", "<!doctype html><meta charset=utf-8><body style=\"font-family:Segoe UI,sans-serif;padding:48px\"><h2>Nie połączono z Gmailem</h2><p>Możesz zamknąć tę kartę.</p>");
            return Err(if e == "access_denied" { "Odmówiono dostępu do Gmaila.".into() } else { format!("Google zwrócił błąd: {e}") });
        }
        let code = query_param(&query, "code").ok_or("Brak kodu autoryzacji w odpowiedzi Google.")?;
        respond(&mut stream, "200 OK", DONE_PAGE);
        return Ok(Grant { code, verifier, redirect });
    }
}

async fn token_request(http: &reqwest::Client, body: String) -> Res<Value> {
    let resp = http
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| format!("Brak połączenia z Google: {e}"))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        let code = v["error"].as_str().unwrap_or("");
        if code == "invalid_grant" {
            return Err("invalid_grant".into());
        }
        return Err(format!("Logowanie Google nie powiodło się: {} {}", code, v["error_description"].as_str().unwrap_or("")).trim().to_string());
    }
    Ok(v)
}

pub struct Token {
    pub access: String,
    pub expires: Instant,
    pub refresh: Option<String>,
}

fn token_from(v: &Value) -> Res<Token> {
    Ok(Token {
        access: v["access_token"].as_str().ok_or("Google nie zwrócił tokenu dostępu.")?.to_string(),
        expires: Instant::now() + Duration::from_secs(v["expires_in"].as_u64().unwrap_or(3600)),
        refresh: v["refresh_token"].as_str().map(str::to_string),
    })
}

pub async fn exchange(http: &reqwest::Client, client_id: &str, secret: &str, g: &Grant) -> Res<Token> {
    let body = form(&[
        ("code", &g.code),
        ("client_id", client_id),
        ("client_secret", secret),
        ("redirect_uri", &g.redirect),
        ("grant_type", "authorization_code"),
        ("code_verifier", &g.verifier),
    ]);
    token_from(&token_request(http, body).await?)
}

pub async fn refresh(http: &reqwest::Client, client_id: &str, secret: &str, refresh_token: &str) -> Res<Token> {
    let body = form(&[("client_id", client_id), ("client_secret", secret), ("refresh_token", refresh_token), ("grant_type", "refresh_token")]);
    token_from(&token_request(http, body).await?)
}

pub async fn revoke(http: &reqwest::Client, token: &str) {
    let _ = http.post(REVOKE_URL).header("Content-Type", "application/x-www-form-urlencoded").body(form(&[("token", token)])).send().await;
}

/// Plik JSON klienta OAuth pobrany z Google Cloud (sekcja „installed”) → (client_id, client_secret).
pub fn parse_client_file(text: &str) -> Res<(String, String)> {
    let v: Value = serde_json::from_str(text).map_err(|_| "To nie jest plik JSON klienta OAuth z Google Cloud.")?;
    let c = if v["installed"].is_object() { &v["installed"] } else { &v["web"] };
    match (c["client_id"].as_str(), c["client_secret"].as_str()) {
        (Some(id), Some(s)) => Ok((id.to_string(), s.to_string())),
        _ => Err("W pliku brak client_id lub client_secret (potrzebny klient typu „Aplikacja komputerowa”).".into()),
    }
}

// --- Wywołania API -----------------------------------------------------------------------------

async fn check(resp: reqwest::Response) -> Res<Value> {
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if status.is_success() {
        return Ok(v);
    }
    let msg = v["error"]["message"].as_str().unwrap_or("").to_string();
    Err(match status.as_u16() {
        401 => "Gmail odrzucił logowanie — połącz ponownie w oknie Poczty.".into(),
        403 | 429 if msg.contains("Quota exceeded") || msg.contains("Rate Limit") || msg.contains("rateLimitExceeded") || status.as_u16() == 429 => {
            "Gmail chwilowo ogranicza liczbę zapytań (limit na minutę dla konta). Odczekaj minutę i spróbuj ponownie.".into()
        }
        403 if msg.contains("has not been used") || msg.contains("disabled") => {
            format!("Gmail API nie jest włączone w projekcie Google Cloud. Włącz je w konsoli i spróbuj ponownie. ({msg})")
        }
        _ => format!("Gmail: {} {}", status.as_u16(), msg).trim().to_string(),
    })
}

async fn get(http: &reqwest::Client, token: &str, path: &str) -> Res<Value> {
    let resp = http.get(format!("{API}{path}")).bearer_auth(token).send().await.map_err(|e| format!("Brak połączenia z Gmailem: {e}"))?;
    check(resp).await
}

async fn post(http: &reqwest::Client, token: &str, path: &str, body: &Value) -> Res<Value> {
    let resp = http.post(format!("{API}{path}")).bearer_auth(token).json(body).send().await.map_err(|e| format!("Brak połączenia z Gmailem: {e}"))?;
    check(resp).await
}

pub async fn profile_email(http: &reqwest::Client, token: &str) -> Res<String> {
    Ok(get(http, token, "/profile").await?["emailAddress"].as_str().unwrap_or("").to_string())
}

fn decode_data(s: &str) -> Res<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).or_else(|_| URL_SAFE.decode(s)).map_err(|e| format!("Uszkodzone dane z Gmaila: {e}"))
}

fn header<'a>(payload: &'a Value, name: &str) -> Option<&'a str> {
    payload["headers"].as_array()?.iter().find(|h| h["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))?["value"].as_str()
}

/// Wszystkie części wiadomości (płasko, w kolejności).
fn parts(payload: &Value) -> Vec<&Value> {
    let mut out = vec![payload];
    let mut i = 0;
    while i < out.len() {
        if let Some(children) = out[i]["parts"].as_array() {
            out.extend(children.iter());
        }
        i += 1;
    }
    out
}

fn is_pdf_part(p: &Value) -> bool {
    let name = p["filename"].as_str().unwrap_or("");
    !name.is_empty() && (name.to_lowercase().ends_with(".pdf") || p["mimeType"].as_str() == Some("application/pdf"))
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MailAttachment {
    pub part_id: String,
    pub filename: String,
    pub size: u64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MailItem {
    pub id: String,
    pub thread_id: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub date: i64,
    pub snippet: String,
    pub unread: bool,
    pub attachments: Vec<MailAttachment>,
}

fn mail_item(m: &Value) -> MailItem {
    let p = &m["payload"];
    let h = |n| header(p, n).unwrap_or("").to_string();
    MailItem {
        id: m["id"].as_str().unwrap_or("").into(),
        thread_id: m["threadId"].as_str().unwrap_or("").into(),
        from: h("From"),
        to: h("To"),
        cc: h("Cc"),
        subject: h("Subject"),
        date: m["internalDate"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0),
        snippet: decode_entities(m["snippet"].as_str().unwrap_or("")),
        unread: m["labelIds"].as_array().is_some_and(|l| l.iter().any(|x| x == "UNREAD")),
        attachments: parts(p)
            .into_iter()
            .filter(|x| is_pdf_part(x))
            .map(|x| MailAttachment {
                part_id: x["partId"].as_str().unwrap_or("").into(),
                filename: x["filename"].as_str().unwrap_or("").into(),
                size: x["body"]["size"].as_u64().unwrap_or(0),
            })
            .collect(),
    }
}

/// Pełne wiadomości już pobrane w tej sesji (treść maila się nie zmienia). Pobranie z format=full
/// kosztuje 60 jednostek limitu Gmaila, a nowe projekty mają tylko 6000 jednostek na minutę na
/// użytkownika — bez pamięci kilka odświeżeń listy w ciągu minuty przekraczało limit.
static MESSAGES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, Value>>> = std::sync::LazyLock::new(Default::default);
const CACHE_MAX: usize = 300;

pub fn clear_cache() {
    MESSAGES.lock().unwrap().clear();
}

async fn message(http: &reqwest::Client, token: &str, id: &str) -> Res<Value> {
    if let Some(m) = MESSAGES.lock().unwrap().get(id) {
        return Ok(m.clone());
    }
    let m = get(http, token, &format!("/messages/{}?format=full", urlencoding::encode(id))).await?;
    let mut cache = MESSAGES.lock().unwrap();
    if cache.len() >= CACHE_MAX {
        cache.clear();
    }
    cache.insert(id.to_string(), m.clone());
    Ok(m)
}

/// Zapytanie zawężone do nadawców z domeny konta (np. `from:@firma.pl`).
pub fn org_query(query: &str, me: &str) -> String {
    match me.rsplit_once('@') {
        Some((_, domain)) if !domain.is_empty() => format!("{} from:@{}", query.trim(), domain.to_lowercase()),
        _ => query.to_string(),
    }
}

/// Maile pasujące do zapytania Gmaila, które mają co najmniej jeden załącznik PDF.
pub async fn list(http: &reqwest::Client, token: &str, query: &str) -> Res<Vec<MailItem>> {
    let ids = get(http, token, &format!("/messages?maxResults={MAX_LIST}&q={}", urlencoding::encode(query))).await?;
    let ids: Vec<String> = ids["messages"].as_array().map(|a| a.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect()).unwrap_or_default();
    let fetched = futures_util::future::join_all(ids.iter().map(|id| message(http, token, id))).await;
    let mut out = vec![];
    for m in fetched {
        let item = mail_item(&m?);
        if !item.attachments.is_empty() {
            out.push(item);
        }
    }
    Ok(out)
}

fn sanitize(name: &str, max: usize) -> String {
    let s: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c }).collect();
    let s: String = s.trim().chars().take(max).collect();
    let s = s.trim_end_matches(['.', ' ']).to_string();
    if s.is_empty() { "dokument".into() } else { s }
}

/// Data UTC (rok, miesiąc, dzień) z milisekund od 1970.
fn ymd(ms: i64) -> (i64, u32, u32) {
    let z = ms.div_euclid(86_400_000) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Zapisuje wskazane załączniki do `base/RRRR-MM-DD temat/` i zwraca ścieżki plików.
pub async fn download(http: &reqwest::Client, token: &str, id: &str, part_ids: &[String], base: &std::path::Path) -> Res<Vec<String>> {
    let m = message(http, token, id).await?;
    let p = &m["payload"];
    let (y, mo, d) = ymd(m["internalDate"].as_str().and_then(|x| x.parse().ok()).unwrap_or(0));
    let subject = header(p, "Subject").unwrap_or("");
    let dir = base.join(format!("{y:04}-{mo:02}-{d:02} {}", sanitize(if subject.trim().is_empty() { "bez tematu" } else { subject }, 60)));
    std::fs::create_dir_all(&dir).map_err(|e| format!("Nie można utworzyć folderu {}: {e}", dir.display()))?;
    let mut out = vec![];
    for part in parts(p).into_iter().filter(|x| is_pdf_part(x)) {
        let pid = part["partId"].as_str().unwrap_or("");
        if !part_ids.iter().any(|x| x == pid) {
            continue;
        }
        let data = match (part["body"]["data"].as_str(), part["body"]["attachmentId"].as_str()) {
            (Some(d), _) => decode_data(d)?,
            (None, Some(att)) => {
                let a = get(http, token, &format!("/messages/{}/attachments/{}", urlencoding::encode(id), urlencoding::encode(att))).await?;
                decode_data(a["data"].as_str().unwrap_or(""))?
            }
            _ => return Err(format!("Załącznik „{}” jest pusty.", part["filename"].as_str().unwrap_or(""))),
        };
        let mut name = sanitize(part["filename"].as_str().unwrap_or("dokument.pdf"), 120);
        if !name.to_lowercase().ends_with(".pdf") {
            name.push_str(".pdf");
        }
        let target = dir.join(&name);
        std::fs::write(&target, &data).map_err(|e| format!("Nie można zapisać {}: {e}", target.display()))?;
        out.push(target.to_string_lossy().to_string());
    }
    Ok(out)
}

// --- Odpowiedź ---------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct Addr {
    pub name: String,
    pub email: String,
}

/// Lista adresów z nagłówka (przecinki poza cudzysłowami i <…>).
pub fn parse_addresses(s: &str) -> Vec<Addr> {
    let mut items = vec![];
    let (mut cur, mut quoted, mut angle) = (String::new(), false, false);
    for c in s.chars() {
        match c {
            '"' if !angle => quoted = !quoted,
            '<' if !quoted => angle = true,
            '>' if !quoted => angle = false,
            ',' | ';' if !quoted && !angle => {
                items.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    items.push(cur);
    items
        .into_iter()
        .filter_map(|it| {
            let it = it.trim();
            let (name, email) = match (it.rfind('<'), it.rfind('>')) {
                (Some(a), Some(b)) if a < b => (it[..a].trim(), it[a + 1..b].trim()),
                _ => ("", it),
            };
            let email = email.trim_matches(|c: char| c == '"' || c.is_whitespace());
            email.contains('@').then(|| Addr { name: name.trim_matches('"').trim().to_string(), email: email.to_string() })
        })
        .collect()
}

/// Adresaci „Odpowiedz wszystkim”: (Do, DW) bez własnego adresu i bez powtórzeń.
pub fn reply_all(from: &str, reply_to: &str, to: &str, cc: &str, me: &str) -> (Vec<Addr>, Vec<Addr>) {
    let is_me = |a: &Addr| a.email.eq_ignore_ascii_case(me);
    let sender = parse_addresses(from);
    let mut first = if sender.iter().any(is_me) {
        // Odpowiedź na własny mail trafia do pierwotnych adresatów.
        parse_addresses(to)
    } else {
        let rt = parse_addresses(reply_to);
        let mut v = if rt.is_empty() { sender } else { rt };
        v.extend(parse_addresses(to));
        v
    };
    let mut seen: Vec<String> = vec![me.to_lowercase()];
    let mut dedup = |list: &mut Vec<Addr>| {
        list.retain(|a| {
            let k = a.email.to_lowercase();
            if seen.contains(&k) {
                false
            } else {
                seen.push(k);
                true
            }
        })
    };
    dedup(&mut first);
    let mut second = parse_addresses(cc);
    dedup(&mut second);
    if first.is_empty() {
        std::mem::swap(&mut first, &mut second);
    }
    (first, second)
}

/// Słowa zakodowane wg RFC 2047 (UTF-8, base64), pocięte na granicach znaków.
fn encode_words(s: &str) -> String {
    if s.is_ascii() {
        return s.to_string();
    }
    let mut words = vec![];
    let mut chunk = String::new();
    for c in s.chars() {
        if chunk.len() + c.len_utf8() > 42 {
            words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(&chunk)));
            chunk.clear();
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(format!("=?UTF-8?B?{}?=", STANDARD.encode(&chunk)));
    }
    words.join("\r\n ")
}

fn format_addr(a: &Addr) -> String {
    if a.name.is_empty() {
        return a.email.clone();
    }
    let name = if a.name.is_ascii() { format!("\"{}\"", a.name.replace(['"', '\\'], "")) } else { encode_words(&a.name) };
    format!("{name} <{}>", a.email)
}

fn format_list(list: &[Addr]) -> String {
    list.iter().map(format_addr).collect::<Vec<_>>().join(",\r\n ")
}

pub fn reply_subject(subject: &str) -> String {
    let s = subject.trim();
    let l = s.to_lowercase();
    if l.starts_with("re:") || l.starts_with("odp:") {
        s.to_string()
    } else {
        format!("Re: {s}")
    }
}

fn wrap_base64(bytes: &[u8]) -> String {
    let b = STANDARD.encode(bytes);
    b.as_bytes().chunks(76).map(|c| std::str::from_utf8(c).unwrap()).collect::<Vec<_>>().join("\r\n")
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&")
}

/// Tekst oryginału do cytowania: część text/plain, a gdy jej brak — HTML bez znaczników.
fn original_text(payload: &Value) -> Option<String> {
    let all = parts(payload);
    let pick = |mime: &str| all.iter().find(|p| p["mimeType"].as_str() == Some(mime) && p["filename"].as_str().unwrap_or("").is_empty())?["body"]["data"].as_str().and_then(|d| decode_data(d).ok());
    if let Some(t) = pick("text/plain") {
        return Some(String::from_utf8_lossy(&t).to_string());
    }
    let html = String::from_utf8_lossy(&pick("text/html")?).to_string();
    let html = html.replace("<br>", "\n").replace("<br/>", "\n").replace("<br />", "\n").replace("</p>", "\n").replace("</div>", "\n");
    let mut out = String::new();
    let mut tag = false;
    for c in html.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if !tag => out.push(c),
            _ => {}
        }
    }
    Some(decode_entities(&out))
}

pub struct Reply {
    pub to: Vec<Addr>,
    pub cc: Vec<Addr>,
    pub subject: String,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub body: String,
    pub files: Vec<(String, Vec<u8>)>,
}

/// Wiadomość MIME (multipart/mixed): treść UTF-8 i załączniki PDF.
pub fn build_mime(r: &Reply, boundary: &str) -> Vec<u8> {
    let mut h = String::new();
    h.push_str("MIME-Version: 1.0\r\n");
    h.push_str(&format!("To: {}\r\n", format_list(&r.to)));
    if !r.cc.is_empty() {
        h.push_str(&format!("Cc: {}\r\n", format_list(&r.cc)));
    }
    h.push_str(&format!("Subject: {}\r\n", encode_words(&r.subject)));
    if let Some(id) = &r.in_reply_to {
        h.push_str(&format!("In-Reply-To: {id}\r\n"));
    }
    if let Some(refs) = &r.references {
        h.push_str(&format!("References: {refs}\r\n"));
    }
    h.push_str(&format!("Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"));
    h.push_str(&format!("--{boundary}\r\nContent-Type: text/plain; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"));
    h.push_str(&wrap_base64(r.body.replace("\r\n", "\n").replace('\n', "\r\n").as_bytes()));
    h.push_str("\r\n");
    for (name, data) in &r.files {
        let ascii: String = name.chars().map(|c| if c.is_ascii() && c != '"' && c != '\\' { c } else { '_' }).collect();
        h.push_str(&format!(
            "--{boundary}\r\nContent-Type: application/pdf; name=\"{}\"\r\nContent-Disposition: attachment; filename=\"{ascii}\"; filename*=UTF-8''{}\r\nContent-Transfer-Encoding: base64\r\n\r\n",
            if name.is_ascii() { ascii.clone() } else { encode_words(name).replace("\r\n ", "") },
            urlencoding::encode(name)
        ));
        h.push_str(&wrap_base64(data));
        h.push_str("\r\n");
    }
    h.push_str(&format!("--{boundary}--\r\n"));
    h.into_bytes()
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct DraftResult {
    pub draft_id: String,
    pub message_id: String,
    pub to: Vec<String>,
    pub cc: Vec<String>,
    pub url: String,
    /// Nie udało się oznaczyć maila etykietą (szkic i tak powstał).
    pub label_warning: Option<String>,
}

/// Szkic „Odpowiedz wszystkim” w wątku maila `id` z plikami `files`; mail dostaje etykietę `done_label`.
pub async fn reply_draft(http: &reqwest::Client, token: &str, me: &str, id: &str, files: &[String], text: &str, done_label: &str) -> Res<DraftResult> {
    let m = message(http, token, id).await?;
    let p = &m["payload"];
    let h = |n| header(p, n).unwrap_or("");
    let (to, cc) = reply_all(h("From"), h("Reply-To"), h("To"), h("Cc"), me);
    if to.is_empty() {
        return Err("Nie znaleziono adresatów odpowiedzi.".into());
    }
    let msg_id = header(p, "Message-ID").or_else(|| header(p, "Message-Id")).map(str::to_string);
    let references = match (header(p, "References"), &msg_id) {
        (Some(r), Some(id)) => Some(format!("{r} {id}")),
        (None, Some(id)) => Some(id.clone()),
        (r, None) => r.map(str::to_string),
    };
    let mut body = text.trim_end().to_string();
    if let Some(orig) = original_text(p) {
        body.push_str(&format!("\n\n{}, {} napisał(a):\n", h("Date"), h("From")));
        for line in orig.replace("\r\n", "\n").trim_end().lines() {
            body.push_str(&format!("> {line}\n"));
        }
    }
    let mut attached = vec![];
    for f in files {
        let data = std::fs::read(f).map_err(|e| format!("Nie można odczytać {f}: {e}"))?;
        let name = std::path::Path::new(f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dokument.pdf".into());
        attached.push((name, data));
    }
    let reply = Reply { to: to.clone(), cc: cc.clone(), subject: reply_subject(h("Subject")), in_reply_to: msg_id, references, body, files: attached };
    let boundary = format!("pdfwb_{}", random_b64(18)?.replace(['-', '_'], "0"));
    let mime = build_mime(&reply, &boundary);
    let thread = m["threadId"].as_str().unwrap_or("");
    let draft = if mime.len() < RAW_LIMIT {
        post(http, token, "/drafts", &json!({ "message": { "raw": URL_SAFE_NO_PAD.encode(&mime), "threadId": thread } })).await?
    } else {
        let outer = format!("pdfwb_up_{}", random_b64(12)?.replace(['-', '_'], "0"));
        let mut body = format!("--{outer}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{}\r\n--{outer}\r\nContent-Type: message/rfc822\r\n\r\n", json!({ "message": { "threadId": thread } })).into_bytes();
        body.extend_from_slice(&mime);
        body.extend_from_slice(format!("\r\n--{outer}--\r\n").as_bytes());
        let resp = http
            .post(format!("{UPLOAD_API}/drafts?uploadType=multipart"))
            .bearer_auth(token)
            .header("Content-Type", format!("multipart/related; boundary={outer}"))
            .body(body)
            .send()
            .await
            .map_err(|e| format!("Brak połączenia z Gmailem: {e}"))?;
        check(resp).await?
    };
    let label_warning = mark_done(http, token, id, done_label).await.err();
    let message_id = draft["message"]["id"].as_str().unwrap_or("").to_string();
    Ok(DraftResult {
        draft_id: draft["id"].as_str().unwrap_or("").into(),
        url: format!("https://mail.google.com/mail/?authuser={}#drafts?compose={}", urlencoding::encode(me), message_id),
        message_id,
        to: to.iter().map(|a| a.email.clone()).collect(),
        cc: cc.iter().map(|a| a.email.clone()).collect(),
        label_warning,
    })
}

/// Wysyła wcześniej utworzony szkic (po kliknięciu „Wyślij” przy widocznych adresatach).
pub async fn send_draft(http: &reqwest::Client, token: &str, draft_id: &str) -> Res<String> {
    let sent = post(http, token, "/drafts/send", &json!({ "id": draft_id })).await?;
    Ok(sent["id"].as_str().unwrap_or("").to_string())
}

async fn mark_done(http: &reqwest::Client, token: &str, id: &str, name: &str) -> Res<()> {
    if name.trim().is_empty() {
        return Ok(());
    }
    let labels = get(http, token, "/labels").await?;
    let existing = labels["labels"].as_array().and_then(|l| l.iter().find(|x| x["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name.trim())))).and_then(|x| x["id"].as_str()).map(str::to_string);
    let label_id = match existing {
        Some(l) => l,
        None => post(http, token, "/labels", &json!({ "name": name.trim(), "labelListVisibility": "labelShow", "messageListVisibility": "show" })).await?["id"]
            .as_str()
            .unwrap_or("")
            .to_string(),
    };
    post(http, token, &format!("/messages/{}/modify", urlencoding::encode(id)), &json!({ "addLabelIds": [label_id] })).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        let a = parse_addresses(r#""Kowalski, Jan" <jan@firma.pl>, anna@x.pl; Żaneta Nowak <ZN@y.pl>"#);
        assert_eq!(
            a,
            vec![
                Addr { name: "Kowalski, Jan".into(), email: "jan@firma.pl".into() },
                Addr { name: "".into(), email: "anna@x.pl".into() },
                Addr { name: "Żaneta Nowak".into(), email: "ZN@y.pl".into() },
            ]
        );
        assert!(parse_addresses("undisclosed-recipients:;").is_empty());
    }

    #[test]
    fn reply_all_recipients() {
        let me = "pawel@ciesielka.eu";
        let (to, cc) = reply_all("Jan <jan@firma.pl>", "", "Pawel@Ciesielka.eu, anna@x.pl", "jan@firma.pl, ola@z.pl", me);
        assert_eq!(to.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["jan@firma.pl", "anna@x.pl"]);
        assert_eq!(cc.iter().map(|a| a.email.as_str()).collect::<Vec<_>>(), ["ola@z.pl"]);
        // Reply-To zastępuje nadawcę.
        let (to, _) = reply_all("Bot <noreply@x.pl>", "biuro@x.pl", me, "", me);
        assert_eq!(to[0].email, "biuro@x.pl");
        // Własny mail: odpowiedź do pierwotnych adresatów.
        let (to, _) = reply_all(me, "", "jan@firma.pl", "", me);
        assert_eq!(to[0].email, "jan@firma.pl");
    }

    #[test]
    fn subject_and_encoding() {
        assert_eq!(reply_subject("Umowa"), "Re: Umowa");
        assert_eq!(reply_subject("RE: Umowa"), "RE: Umowa");
        let enc = encode_words("Umowa o dzieło — prośba o podpis i odesłanie skanu dokumentów");
        for w in enc.split("\r\n ") {
            assert!(w.len() <= 75, "{w}");
            let inner = w.trim_start_matches("=?UTF-8?B?").trim_end_matches("?=");
            std::str::from_utf8(&STANDARD.decode(inner).unwrap()).unwrap();
        }
    }

    #[test]
    fn mime_structure() {
        let r = Reply {
            to: vec![Addr { name: "Łukasz".into(), email: "l@x.pl".into() }],
            cc: vec![],
            subject: "Re: Umowa".into(),
            in_reply_to: Some("<abc@mail>".into()),
            references: Some("<abc@mail>".into()),
            body: "W załączeniu.\nPozdrawiam".into(),
            files: vec![("umowa (podpisany).pdf".into(), b"%PDF-1.7 test".to_vec())],
        };
        let m = String::from_utf8(build_mime(&r, "B")).unwrap();
        assert!(m.contains("To: =?UTF-8?B?"));
        assert!(m.contains("In-Reply-To: <abc@mail>\r\n"));
        assert!(m.contains("filename*=UTF-8''umowa%20%28podpisany%29.pdf"));
        assert!(m.contains(&STANDARD.encode(b"%PDF-1.7 test")));
        assert!(m.ends_with("--B--\r\n"));
    }

    #[test]
    fn helpers() {
        assert_eq!(ymd(0), (1970, 1, 1));
        assert_eq!(ymd(1_794_268_800_000), (2026, 11, 10));
        assert_eq!(sanitize(" Umowa: nr 1/2026? ", 60), "Umowa_ nr 1_2026_");
        assert_eq!(org_query("in:inbox", "Pawel@Firma.PL"), "in:inbox from:@firma.pl");
        assert_eq!(org_query("in:inbox", "bez-domeny"), "in:inbox");
        assert_eq!(query_param("state=a%2Bb&code=4%2F0x", "code").as_deref(), Some("4/0x"));
        let (id, secret) = parse_client_file(r#"{"installed":{"client_id":"1.apps","client_secret":"s"}}"#).unwrap();
        assert_eq!((id.as_str(), secret.as_str()), ("1.apps", "s"));
    }

    #[test]
    fn pdf_parts() {
        let m = json!({
            "id": "m1", "threadId": "t1", "internalDate": "1794268800000", "labelIds": ["UNREAD"],
            "payload": { "headers": [{"name": "Subject", "value": "Umowa"}, {"name": "From", "value": "jan@firma.pl"}],
                "parts": [
                    {"partId": "0", "mimeType": "text/plain", "filename": "", "body": {"data": URL_SAFE_NO_PAD.encode("Proszę o podpis")}},
                    {"partId": "1", "mimeType": "application/pdf", "filename": "umowa.pdf", "body": {"attachmentId": "a", "size": 1234}},
                    {"partId": "2", "mimeType": "image/png", "filename": "logo.png", "body": {"attachmentId": "b"}}
                ] }
        });
        let it = mail_item(&m);
        assert_eq!(it.attachments.len(), 1);
        assert_eq!(it.attachments[0].part_id, "1");
        assert!(it.unread);
        assert_eq!(original_text(&m["payload"]).as_deref(), Some("Proszę o podpis"));
    }
}
