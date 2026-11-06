//! Podpis elektroniczny PAdES certyfikatem z magazynu Windows (np. SimplySign Desktop) oraz odczyt
//! i sprawdzenie integralności podpisów w PDF.
//!
//! Podpis dopisywany jest jako aktualizacja przyrostowa: oryginalne bajty pliku zostają nietknięte,
//! więc wcześniejsze podpisy pozostają ważne. Skrót liczy i podpisuje CryptoAPI Windows — klucz
//! prywatny nigdy nie trafia do programu (przy SimplySign podpis wykonuje chmura Certum).

use lopdf::{Dictionary, Document, Object, ObjectId, StringFormat};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::null_mut;
use windows::core::{w, PCSTR, PCWSTR, PSTR};
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Security::Cryptography::*;

type Res<T> = Result<T, String>;

mod ltv;
pub mod trust;
pub use trust::{issuer_countries, check_trust, TrustInfo};

const ENC: CERT_QUERY_ENCODING_TYPE = CERT_QUERY_ENCODING_TYPE(X509_ASN_ENCODING.0 | PKCS_7_ASN_ENCODING.0);
const OID_SHA256: &[u8] = b"2.16.840.1.101.3.4.2.1\0";
const OID_SIGNING_CERT_V2: &[u8] = b"1.2.840.113549.1.9.16.2.47\0";
const OID_QC_STATEMENTS: &[u8] = b"1.3.6.1.5.5.7.1.3\0";
/// OID id-aa-timeStampToken (1.2.840.113549.1.9.16.2.14) w DER.
const DER_OID_TST: [u8; 13] = [0x06, 0x0B, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x09, 0x10, 0x02, 0x0E];
/// Serwer znaczników czasu Certum (bezpłatny, RFC 3161).
pub const DEFAULT_TSA: &str = "http://time.certum.pl";
/// Miejsce zarezerwowane na podpis CMS (certyfikaty + znacznik czasu mieszczą się z zapasem).
const CONTENTS_BYTES: usize = 30_000;
const BR_PLACEHOLDER: &[u8] = b"[0 0000000000 0000000000 0000000000]";

// --- Uchwyty CryptoAPI ---------------------------------------------------------------------------

struct Cert(*const CERT_CONTEXT);

impl Drop for Cert {
    fn drop(&mut self) {
        unsafe {
            let _ = CertFreeCertificateContext(Some(self.0));
        }
    }
}

unsafe fn blob<'a>(b: &CRYPT_INTEGER_BLOB) -> &'a [u8] {
    if b.pbData.is_null() || b.cbData == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(b.pbData, b.cbData as usize)
    }
}

impl Cert {
    fn der(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts((*self.0).pbCertEncoded, (*self.0).cbCertEncoded as usize) }
    }
    fn info(&self) -> &CERT_INFO {
        unsafe { &*(*self.0).pCertInfo }
    }
    fn name(&self, issuer: bool) -> String {
        unsafe {
            let flags = if issuer { CERT_NAME_ISSUER_FLAG } else { 0 };
            let n = CertGetNameStringW(self.0, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, None, None);
            let mut buf = vec![0u16; n.max(1) as usize];
            CertGetNameStringW(self.0, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, None, Some(&mut buf));
            String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string()
        }
    }
    fn id(&self) -> String {
        hex::encode(Sha256::digest(self.der()))
    }
    fn qualified(&self) -> bool {
        unsafe {
            let i = self.info();
            if i.rgExtension.is_null() || i.cExtension == 0 {
                return false;
            }
            let exts = std::slice::from_raw_parts(i.rgExtension, i.cExtension as usize);
            !CertFindExtension(PCSTR(OID_QC_STATEMENTS.as_ptr()), exts).is_null()
        }
    }
    fn has_private_key(&self) -> bool {
        unsafe {
            let mut n = 0u32;
            CertGetCertificateContextProperty(self.0, CERT_KEY_PROV_INFO_PROP_ID, None, &mut n).is_ok()
                || CertGetCertificateContextProperty(self.0, CERT_NCRYPT_KEY_HANDLE_PROP_ID, None, &mut n).is_ok()
        }
    }
}

struct Store(HCERTSTORE);

impl Drop for Store {
    fn drop(&mut self) {
        unsafe {
            let _ = CertCloseStore(Some(self.0), 0);
        }
    }
}

struct Msg(*mut c_void);

impl Drop for Msg {
    fn drop(&mut self) {
        unsafe {
            let _ = CryptMsgClose(Some(self.0));
        }
    }
}

struct Key {
    handle: HCRYPTPROV_OR_NCRYPT_KEY_HANDLE,
    spec: CERT_KEY_SPEC,
    free: bool,
}

impl Drop for Key {
    fn drop(&mut self) {
        if !self.free {
            return;
        }
        unsafe {
            if self.spec == CERT_NCRYPT_KEY_SPEC {
                let _ = NCryptFreeObject(NCRYPT_HANDLE(self.handle.0));
            } else {
                let _ = CryptReleaseContext(self.handle.0, 0);
            }
        }
    }
}

fn ft_ms(ft: &FILETIME) -> i64 {
    let v = (((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64) as i64;
    (v - 116_444_736_000_000_000) / 10_000
}

/// Osobisty magazyn certyfikatów bieżącego użytkownika („Osobisty”) — tam SimplySign Desktop
/// rejestruje certyfikaty z karty wirtualnej.
fn open_my() -> Res<Store> {
    unsafe {
        CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            CERT_QUERY_ENCODING_TYPE(0),
            None,
            CERT_OPEN_STORE_FLAGS(CERT_SYSTEM_STORE_CURRENT_USER | CERT_STORE_READONLY_FLAG.0 | CERT_STORE_OPEN_EXISTING_FLAG.0),
            Some(w!("MY").as_ptr() as *const c_void),
        )
        .map(Store)
        .map_err(|e| format!("Nie można otworzyć magazynu certyfikatów: {e}"))
    }
}

fn certs_with_key() -> Res<Vec<Cert>> {
    let store = open_my()?;
    let mut out = Vec::new();
    unsafe {
        let mut prev: *mut CERT_CONTEXT = null_mut();
        loop {
            prev = CertEnumCertificatesInStore(store.0, if prev.is_null() { None } else { Some(prev) });
            if prev.is_null() {
                break;
            }
            let c = Cert(CertDuplicateCertificateContext(Some(prev)));
            if c.has_private_key() {
                out.push(c);
            }
        }
    }
    Ok(out)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningCert {
    pub id: String,
    pub name: String,
    pub issuer: String,
    pub valid_from: i64,
    pub valid_to: i64,
    pub qualified: bool,
}

/// Certyfikaty z kluczem prywatnym, którymi można podpisać (kwalifikowane i najdłużej ważne najpierw).
pub fn list_certs() -> Res<Vec<SigningCert>> {
    let mut list: Vec<SigningCert> = certs_with_key()?
        .iter()
        .map(|c| SigningCert {
            id: c.id(),
            name: c.name(false),
            issuer: c.name(true),
            valid_from: ft_ms(&c.info().NotBefore),
            valid_to: ft_ms(&c.info().NotAfter),
            qualified: c.qualified(),
        })
        .collect();
    list.sort_by(|a, b| b.qualified.cmp(&a.qualified).then(b.valid_to.cmp(&a.valid_to)));
    Ok(list)
}

fn find_cert(id: &str) -> Res<Cert> {
    certs_with_key()?
        .into_iter()
        .find(|c| c.id() == id)
        .ok_or_else(|| "Nie znaleziono wybranego certyfikatu. Uruchom SimplySign Desktop, zaloguj się tokenem i spróbuj ponownie.".into())
}

fn acquire_key(cert: &Cert) -> Res<Key> {
    let mut handle = HCRYPTPROV_OR_NCRYPT_KEY_HANDLE(0);
    let mut spec = CERT_KEY_SPEC(0);
    let mut free = windows::core::BOOL(0);
    unsafe {
        CryptAcquireCertificatePrivateKey(cert.0, CRYPT_ACQUIRE_PREFER_NCRYPT_KEY_FLAG, None, &mut handle, Some(&mut spec), Some(&mut free))
            .map_err(|e| format!("Brak dostępu do klucza prywatnego certyfikatu — czy SimplySign Desktop jest uruchomiony i zalogowany? ({e})"))?;
    }
    Ok(Key { handle, spec, free: free.as_bool() })
}

/// Certyfikaty pośrednie (bez głównego) — dołączane do podpisu, żeby weryfikator zbudował łańcuch.
fn chain_certs(cert: &Cert) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    unsafe {
        let mut para: CERT_CHAIN_PARA = std::mem::zeroed();
        para.cbSize = std::mem::size_of::<CERT_CHAIN_PARA>() as u32;
        let mut ctx: *mut CERT_CHAIN_CONTEXT = null_mut();
        if CertGetCertificateChain(None, cert.0, None, None, &para, 0, None, &mut ctx).is_err() || ctx.is_null() {
            return out;
        }
        if (*ctx).cChain > 0 {
            let simple = *(*ctx).rgpChain;
            for i in 1..(*simple).cElement as usize {
                let el = *(*simple).rgpElement.add(i);
                let c = (*el).pCertContext;
                let info = &*(*c).pCertInfo;
                if blob(&info.Subject) == blob(&info.Issuer) {
                    continue; // certyfikat główny — weryfikator ma go we własnej liście zaufanych
                }
                out.push(std::slice::from_raw_parts((*c).pbCertEncoded, (*c).cbCertEncoded as usize).to_vec());
            }
        }
        CertFreeCertificateChain(ctx);
    }
    out
}

// --- DER -----------------------------------------------------------------------------------------

fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut v = vec![tag];
    let n = content.len();
    if n < 0x80 {
        v.push(n as u8);
    } else {
        let b: Vec<u8> = n.to_be_bytes().iter().copied().skip_while(|x| *x == 0).collect();
        v.push(0x80 | b.len() as u8);
        v.extend(b);
    }
    v.extend_from_slice(content);
    v
}

/// Długość całej struktury DER (zawartość /Contents jest dopełniona zerami).
fn der_len(d: &[u8]) -> Option<usize> {
    if d.len() < 2 {
        return None;
    }
    let l = d[1] as usize;
    if l < 0x80 {
        return Some(2 + l);
    }
    let n = l & 0x7f;
    if n == 0 || n > 4 || d.len() < 2 + n {
        return None;
    }
    Some(2 + n + d[2..2 + n].iter().fold(0usize, |a, b| (a << 8) | *b as usize))
}

/// Atrybut signing-certificate-v2 (wymagany przez PAdES): skrót SHA-256 certyfikatu + wystawca i numer.
fn signing_certificate_v2(cert: &Cert) -> Vec<u8> {
    let hash = Sha256::digest(cert.der());
    let info = cert.info();
    let issuer = unsafe { blob(&info.Issuer) };
    // CryptoAPI trzyma numer seryjny od najmłodszego bajtu.
    let serial: Vec<u8> = unsafe { blob(&info.SerialNumber) }.iter().rev().copied().collect();
    let general_names = der(0x30, &der(0xA4, issuer));
    let issuer_serial = der(0x30, &[general_names, der(0x02, &serial)].concat());
    let ess_cert_id = der(0x30, &[der(0x04, &hash), issuer_serial].concat());
    der(0x30, &der(0x30, &ess_cert_id))
}

// --- CMS ------------------------------------------------------------------------------------------

unsafe fn msg_param(msg: *const c_void, param: u32) -> Res<Vec<u8>> {
    let mut n = 0u32;
    CryptMsgGetParam(msg, param, 0, None, &mut n).map_err(|e| e.to_string())?;
    let mut v = vec![0u8; n as usize];
    CryptMsgGetParam(msg, param, 0, Some(v.as_mut_ptr() as *mut c_void), &mut n).map_err(|e| e.to_string())?;
    v.truncate(n as usize);
    Ok(v)
}

/// Jak `msg_param`, ale bufor wyrównany do 8 bajtów (gdy wynikiem jest struktura, np. CERT_INFO).
unsafe fn msg_param_struct(msg: *const c_void, param: u32) -> Res<Vec<u64>> {
    let mut n = 0u32;
    CryptMsgGetParam(msg, param, 0, None, &mut n).map_err(|e| e.to_string())?;
    let mut v = vec![0u64; (n as usize).div_ceil(8)];
    CryptMsgGetParam(msg, param, 0, Some(v.as_mut_ptr() as *mut c_void), &mut n).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Znacznik czasu RFC 3161 dla wartości podpisu (atrybut niepodpisany signature-time-stamp).
fn timestamp(url: &str, signature: &[u8]) -> Res<Vec<u8>> {
    let wurl: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut nonce = Sha256::digest([&seed.to_le_bytes()[..], signature].concat())[..8].to_vec();
    nonce[0] &= 0x7f; // dodatnia liczba całkowita
    let para = CRYPT_TIMESTAMP_PARA {
        pszTSAPolicyId: PCSTR::null(),
        fRequestCerts: true.into(),
        Nonce: CRYPT_INTEGER_BLOB { cbData: nonce.len() as u32, pbData: nonce.as_mut_ptr() },
        cExtension: 0,
        rgExtension: null_mut(),
    };
    unsafe {
        let mut ctx: *mut CRYPT_TIMESTAMP_CONTEXT = null_mut();
        let mut signer: *mut CERT_CONTEXT = null_mut();
        let mut store = HCERTSTORE(null_mut());
        CryptRetrieveTimeStamp(
            PCWSTR(wurl.as_ptr()),
            TIMESTAMP_VERIFY_CONTEXT_SIGNATURE,
            30_000,
            PCSTR(OID_SHA256.as_ptr()),
            Some(&para),
            signature,
            &mut ctx,
            &mut signer,
            Some(&mut store),
        )
        .map_err(|e| format!("Nie udało się pobrać znacznika czasu z {url} ({e}). Sprawdź połączenie z internetem albo podpisz bez znacznika czasu."))?;
        let token = std::slice::from_raw_parts((*ctx).pbEncoded, (*ctx).cbEncoded as usize).to_vec();
        CryptMemFree(Some(ctx as *const c_void));
        if !signer.is_null() {
            let _ = CertFreeCertificateContext(Some(signer));
        }
        if !store.0.is_null() {
            let _ = CertCloseStore(Some(store), 0);
        }
        Ok(token)
    }
}

/// Pobiera znacznik czasu dla wartości podpisu i dopisuje go do podpisu CMS jako atrybut niepodpisany.
fn add_timestamp(cms: &[u8], url: &str) -> Res<Vec<u8>> {
    unsafe {
        let msg = Msg(CryptMsgOpenToDecode(ENC.0, CMSG_DETACHED_FLAG, 0, None, None, None));
        if msg.0.is_null() {
            return Err("Nie można otworzyć podpisu CMS.".into());
        }
        CryptMsgUpdate(msg.0, Some(cms), true).map_err(|e| e.to_string())?;
        let signature = msg_param(msg.0, CMSG_ENCRYPTED_DIGEST)?;
        let token = timestamp(url, &signature)?;
        let mut a = der(0x30, &[&DER_OID_TST[..], &der(0x31, &token)].concat());
        let para = CMSG_CTRL_ADD_SIGNER_UNAUTH_ATTR_PARA {
            cbSize: std::mem::size_of::<CMSG_CTRL_ADD_SIGNER_UNAUTH_ATTR_PARA>() as u32,
            dwSignerIndex: 0,
            blob: CRYPT_INTEGER_BLOB { cbData: a.len() as u32, pbData: a.as_mut_ptr() },
        };
        CryptMsgControl(msg.0, 0, CMSG_CTRL_ADD_SIGNER_UNAUTH_ATTR, Some(&para as *const _ as *const c_void)).map_err(|e| e.to_string())?;
        msg_param(msg.0, CMSG_ENCODED_MESSAGE)
    }
}

/// Podpis CMS (CAdES) odłączony: SHA-256, atrybut signing-certificate-v2, certyfikaty, opcjonalnie znacznik czasu.
fn cms_sign(data: &[u8], cert: &Cert, key: &Key, extra_certs: &[Vec<u8>], tsa: Option<&str>) -> Res<Vec<u8>> {
    let mut attr_der = signing_certificate_v2(cert);
    let mut ders: Vec<Vec<u8>> = std::iter::once(cert.der().to_vec()).chain(extra_certs.iter().cloned()).collect();
    let cms = unsafe {
        let mut attr_blob = CRYPT_INTEGER_BLOB { cbData: attr_der.len() as u32, pbData: attr_der.as_mut_ptr() };
        let mut attr = CRYPT_ATTRIBUTE { pszObjId: PSTR(OID_SIGNING_CERT_V2.as_ptr() as *mut u8), cValue: 1, rgValue: &mut attr_blob };
        let mut signer: CMSG_SIGNER_ENCODE_INFO = std::mem::zeroed();
        signer.cbSize = std::mem::size_of::<CMSG_SIGNER_ENCODE_INFO>() as u32;
        signer.pCertInfo = (*cert.0).pCertInfo;
        if key.spec == CERT_NCRYPT_KEY_SPEC {
            signer.Anonymous.hNCryptKey = NCRYPT_KEY_HANDLE(key.handle.0);
        } else {
            signer.Anonymous.hCryptProv = key.handle.0;
        }
        signer.dwKeySpec = key.spec.0;
        signer.HashAlgorithm.pszObjId = PSTR(OID_SHA256.as_ptr() as *mut u8);
        signer.cAuthAttr = 1;
        signer.rgAuthAttr = &mut attr;
        let mut blobs: Vec<CRYPT_INTEGER_BLOB> =
            ders.iter_mut().map(|d| CRYPT_INTEGER_BLOB { cbData: d.len() as u32, pbData: d.as_mut_ptr() }).collect();
        let mut info: CMSG_SIGNED_ENCODE_INFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<CMSG_SIGNED_ENCODE_INFO>() as u32;
        info.cSigners = 1;
        info.rgSigners = &mut signer;
        info.cCertEncoded = blobs.len() as u32;
        info.rgCertEncoded = blobs.as_mut_ptr();
        let msg = Msg(CryptMsgOpenToEncode(ENC.0, CMSG_DETACHED_FLAG, CMSG_SIGNED, &info as *const _ as *const c_void, PCSTR::null(), None));
        if msg.0.is_null() {
            return Err(format!("Nie można przygotować podpisu: {}", windows::core::Error::from_thread()));
        }
        // Tu dostawca klucza (SimplySign) wykonuje podpis.
        CryptMsgUpdate(msg.0, Some(data), true).map_err(|e| format!("Podpisanie nie powiodło się: {e}"))?;
        msg_param(msg.0, CMSG_CONTENT_PARAM)?
    };
    match tsa {
        Some(url) => add_timestamp(&cms, url),
        None => Ok(cms),
    }
}

struct Verified {
    ok: bool,
    signer: Option<String>,
    issuer: Option<String>,
    qualified: bool,
    problem: Option<String>,
}

/// Sprawdza matematycznie podpis CMS względem podpisanych bajtów (bez łańcucha zaufania).
fn verify(cms: &[u8], data: &[u8]) -> Verified {
    let mut v = Verified { ok: false, signer: None, issuer: None, qualified: false, problem: None };
    unsafe {
        let msg = Msg(CryptMsgOpenToDecode(ENC.0, CMSG_DETACHED_FLAG, 0, None, None, None));
        if msg.0.is_null() {
            v.problem = Some("Nie można odczytać podpisu.".into());
            return v;
        }
        if let Err(e) = CryptMsgUpdate(msg.0, Some(cms), true).and_then(|_| CryptMsgUpdate(msg.0, Some(data), true)) {
            v.problem = Some(format!("Uszkodzona struktura podpisu ({e})."));
            return v;
        }
        let info = match msg_param_struct(msg.0, CMSG_SIGNER_CERT_INFO_PARAM) {
            Ok(i) => i,
            Err(e) => {
                v.problem = Some(format!("Brak danych podpisującego ({e})."));
                return v;
            }
        };
        let store = match CertOpenStore(CERT_STORE_PROV_MSG, ENC, None, CERT_OPEN_STORE_FLAGS(0), Some(msg.0 as *const c_void)) {
            Ok(s) => Store(s),
            Err(e) => {
                v.problem = Some(e.to_string());
                return v;
            }
        };
        let c = CertGetSubjectCertificateFromStore(store.0, ENC, info.as_ptr() as *const CERT_INFO);
        if c.is_null() {
            v.problem = Some("Podpis nie zawiera certyfikatu podpisującego.".into());
            return v;
        }
        let cert = Cert(c);
        v.signer = Some(cert.name(false));
        v.issuer = Some(cert.name(true));
        v.qualified = cert.qualified();
        match CryptMsgControl(msg.0, 0, CMSG_CTRL_VERIFY_SIGNATURE, Some((*cert.0).pCertInfo as *const c_void)) {
            Ok(()) => v.ok = true,
            Err(e) => v.problem = Some(format!("Podpis nie zgadza się z treścią — dokument zmieniono po podpisaniu ({e}).")),
        }
    }
    v
}

// --- Odczyt podpisów z PDF ------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInfo {
    /// "signature" albo "timestamp" (znacznik czasu dokumentu).
    pub kind: &'static str,
    pub signer: Option<String>,
    pub issuer: Option<String>,
    /// Data z /M w formacie PDF („D:20261106143205+01'00'”).
    pub time: Option<String>,
    pub reason: Option<String>,
    /// Podpis zgadza się z podpisanymi bajtami.
    pub intact: bool,
    /// Podpis obejmuje cały plik (po nim nic nie dopisano).
    pub covers_whole: bool,
    pub timestamped: bool,
    pub qualified: bool,
    pub problem: Option<String>,
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

fn parse_byte_range(s: &[u8]) -> Option<[usize; 4]> {
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    if s.get(i) != Some(&b'[') {
        return None;
    }
    i += 1;
    let mut nums = [0usize; 4];
    for n in nums.iter_mut() {
        while i < s.len() && s[i].is_ascii_whitespace() {
            i += 1;
        }
        let start = i;
        while i < s.len() && s[i].is_ascii_digit() {
            i += 1;
        }
        *n = std::str::from_utf8(&s[start..i]).ok()?.parse().ok()?;
    }
    Some(nums)
}

fn decode_hex(s: &[u8]) -> Vec<u8> {
    let digits: Vec<u8> = s.iter().filter_map(|c| (*c as char).to_digit(16).map(|d| d as u8)).collect();
    digits.chunks(2).map(|p| (p[0] << 4) | p.get(1).copied().unwrap_or(0)).collect()
}

fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xFE, 0xFF]) {
        let units: Vec<u16> = b[2..].chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)])).collect();
        String::from_utf16_lossy(&units)
    } else if let Ok(s) = std::str::from_utf8(b) {
        s.to_string()
    } else {
        b.iter().map(|c| *c as char).collect()
    }
}

/// Wartość tekstowa klucza słownika PDF (literał `( )` albo `< >`) w podanym fragmencie.
fn dict_string(dict: &[u8], key: &[u8]) -> Option<String> {
    let mut from = 0;
    while let Some(p) = find(&dict[from..], key) {
        let mut i = from + p + key.len();
        from = i;
        if dict.get(i).is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue; // np. /M w /MDP
        }
        while i < dict.len() && dict[i].is_ascii_whitespace() {
            i += 1;
        }
        match dict.get(i) {
            Some(b'(') => {
                let mut out = Vec::new();
                let mut depth = 0;
                i += 1;
                while i < dict.len() {
                    let c = dict[i];
                    match c {
                        b'\\' => {
                            i += 1;
                            match dict.get(i) {
                                Some(b'n') => out.push(b'\n'),
                                Some(b'r') => out.push(b'\r'),
                                Some(b't') => out.push(b'\t'),
                                Some(d) if d.is_ascii_digit() => {
                                    let mut v = 0u32;
                                    let mut k = 0;
                                    while k < 3 && dict.get(i).is_some_and(|d| (b'0'..=b'7').contains(d)) {
                                        v = v * 8 + (dict[i] - b'0') as u32;
                                        i += 1;
                                        k += 1;
                                    }
                                    out.push(v as u8);
                                    continue;
                                }
                                Some(d) => out.push(*d),
                                None => break,
                            }
                        }
                        b'(' => {
                            depth += 1;
                            out.push(c);
                        }
                        b')' if depth == 0 => break,
                        b')' => {
                            depth -= 1;
                            out.push(c);
                        }
                        _ => out.push(c),
                    }
                    i += 1;
                }
                return Some(decode_text(&out));
            }
            Some(b'<') if dict.get(i + 1) != Some(&b'<') => {
                let end = find(&dict[i..], b">")? + i;
                return Some(decode_text(&decode_hex(&dict[i + 1..end])));
            }
            _ => continue,
        }
    }
    None
}

/// Podpis znaleziony w pliku: wartość /Contents, podpisane bajty i fragment słownika.
pub(crate) struct RawSig {
    pub cms: Vec<u8>,
    pub data: Vec<u8>,
    pub dict: Vec<u8>,
    pub covers_whole: bool,
    /// Znacznik czasu dokumentu (/SubFilter /ETSI.RFC3161): /Contents to token RFC 3161.
    pub doc_timestamp: bool,
}

/// Wszystkie /ByteRange w pliku (szybkie dla plików bez podpisów).
pub(crate) fn raw_signatures(bytes: &[u8]) -> Vec<RawSig> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut from = 0;
    while let Some(p) = find(&bytes[from..], b"/ByteRange") {
        let pos = from + p;
        from = pos + 10;
        let Some(br) = parse_byte_range(&bytes[from..bytes.len().min(from + 200)]) else { continue };
        let [a, b, c, d] = br;
        if a != 0 || b >= c || c + d > bytes.len() || bytes[b] != b'<' || bytes[c - 1] != b'>' || !seen.insert(br) {
            continue;
        }
        let raw = decode_hex(&bytes[b + 1..c - 1]);
        let cms = raw[..der_len(&raw).unwrap_or(raw.len()).min(raw.len())].to_vec();
        let data = [&bytes[..b], &bytes[c..c + d]].concat();
        // Fragment słownika podpisu wokół /Contents (bez samej wartości podpisu).
        let start = rfind(&bytes[b.saturating_sub(4096)..b], b"obj").map(|q| b.saturating_sub(4096) + q).unwrap_or(b.saturating_sub(4096));
        let tail = &bytes[c..bytes.len().min(c + 4096)];
        let tail = &tail[..find(tail, b"endobj").unwrap_or(tail.len())];
        let dict = [&bytes[start..b], tail].concat();
        out.push(RawSig {
            doc_timestamp: find(&dict, b"ETSI.RFC3161").is_some(),
            covers_whole: bytes[c + d..].iter().all(|x| x.is_ascii_whitespace() || *x == 0),
            cms,
            data,
            dict,
        });
    }
    out
}

/// Znacznik czasu dokumentu: token musi dotyczyć dokładnie podpisanych bajtów.
fn verify_doc_timestamp(token: &[u8], data: &[u8]) -> Verified {
    let mut v = Verified { ok: false, signer: None, issuer: None, qualified: false, problem: None };
    unsafe {
        let mut ctx: *mut CRYPT_TIMESTAMP_CONTEXT = null_mut();
        let mut signer: *mut CERT_CONTEXT = null_mut();
        let mut store = HCERTSTORE(null_mut());
        match CryptVerifyTimeStampSignature(token, Some(data), None, &mut ctx, &mut signer, Some(&mut store)) {
            Ok(()) => v.ok = true,
            Err(e) => v.problem = Some(format!("Znacznik czasu nie zgadza się z treścią — dokument zmieniono po jego dodaniu ({e}).")),
        }
        if !signer.is_null() {
            let c = Cert(signer);
            v.signer = Some(c.name(false));
            v.issuer = Some(c.name(true));
        }
        if !ctx.is_null() {
            CryptMemFree(Some(ctx as *const c_void));
        }
        if !store.0.is_null() {
            let _ = CertCloseStore(Some(store), 0);
        }
    }
    v
}

/// Wszystkie podpisy w pliku z oceną integralności. Szybkie dla plików bez podpisów.
pub fn inspect(bytes: &[u8]) -> Vec<SignatureInfo> {
    raw_signatures(bytes)
        .into_iter()
        .map(|r| {
            let v = if r.doc_timestamp { verify_doc_timestamp(&r.cms, &r.data) } else { verify(&r.cms, &r.data) };
            SignatureInfo {
                kind: if r.doc_timestamp { "timestamp" } else { "signature" },
                signer: v.signer.or_else(|| dict_string(&r.dict, b"/Name")),
                issuer: v.issuer,
                time: dict_string(&r.dict, b"/M"),
                reason: dict_string(&r.dict, b"/Reason"),
                intact: v.ok,
                covers_whole: r.covers_whole,
                timestamped: r.doc_timestamp || r.cms.windows(DER_OID_TST.len()).any(|w| w == DER_OID_TST),
                qualified: v.qualified,
                problem: v.problem,
            }
        })
        .collect()
}

// --- Zapis podpisu do PDF -------------------------------------------------------------------------

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Visible {
    /// Numer strony od 1.
    pub page: u32,
    /// Prostokąt w punktach PDF [x0, y0, x1, y1].
    pub rect: [f32; 4],
    /// Wiersze tekstu; drugi wiersz (nazwisko) pogrubiony.
    pub lines: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SignOptions {
    pub cert_id: String,
    /// Czas podpisu w formacie PDF („D:20261106143205+01'00'”).
    pub signing_time: String,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub timestamp: bool,
    /// Dane do długoterminowej weryfikacji (DSS) i znacznik czasu dokumentu — PAdES-LTA.
    #[serde(default)]
    pub ltv: bool,
    pub visible: Option<Visible>,
}

fn fmt_num(f: f32) -> String {
    if f.fract() == 0.0 && f.abs() < 1e9 {
        format!("{}", f as i64)
    } else {
        let s = format!("{f:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn ser(o: &Object, out: &mut Vec<u8>) {
    match o {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Boolean(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Integer(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(f) => out.extend_from_slice(fmt_num(*f).as_bytes()),
        Object::Name(n) => {
            out.push(b'/');
            for &c in n {
                if c > 0x20 && c < 0x7f && !b"()<>[]{}/%#".contains(&c) {
                    out.push(c);
                } else {
                    out.extend_from_slice(format!("#{c:02X}").as_bytes());
                }
            }
        }
        Object::String(s, _) => {
            out.push(b'<');
            out.extend_from_slice(hex::encode_upper(s).as_bytes());
            out.push(b'>');
        }
        Object::Array(a) => {
            out.push(b'[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                ser(x, out);
            }
            out.push(b']');
        }
        Object::Dictionary(d) => ser_dict(d, out),
        Object::Reference((id, gen)) => out.extend_from_slice(format!("{id} {gen} R").as_bytes()),
        Object::Stream(s) => {
            let mut d = s.dict.clone();
            d.set("Length", Object::Integer(s.content.len() as i64));
            ser_dict(&d, out);
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&s.content);
            out.extend_from_slice(b"\nendstream");
        }
    }
}

fn ser_dict(d: &Dictionary, out: &mut Vec<u8>) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        out.push(b' ');
        ser(&Object::Name(k.clone()), out);
        out.push(b' ');
        ser(v, out);
    }
    out.extend_from_slice(b" >>");
}

fn dict_bytes(d: &Dictionary) -> Vec<u8> {
    let mut v = Vec::new();
    ser_dict(d, &mut v);
    v
}

/// Ciąg tekstowy PDF: ASCII wprost, inaczej UTF-16BE ze znacznikiem BOM.
fn text_string(s: &str) -> Object {
    if s.is_ascii() {
        Object::String(s.as_bytes().to_vec(), StringFormat::Literal)
    } else {
        let mut b = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            b.extend_from_slice(&u.to_be_bytes());
        }
        Object::String(b, StringFormat::Hexadecimal)
    }
}

fn name(n: &str) -> Object {
    Object::Name(n.as_bytes().to_vec())
}

/// Polskie znaki w kodowaniu czcionki wyglądu podpisu (WinAnsi + /Differences od 128).
const PL_GLYPHS: [(char, &str); 16] = [
    ('Ą', "Aogonek"),
    ('ą', "aogonek"),
    ('Ć', "Cacute"),
    ('ć', "cacute"),
    ('Ę', "Eogonek"),
    ('ę', "eogonek"),
    ('Ł', "Lslash"),
    ('ł', "lslash"),
    ('Ń', "Nacute"),
    ('ń', "nacute"),
    ('Ś', "Sacute"),
    ('ś', "sacute"),
    ('Ź', "Zacute"),
    ('ź', "zacute"),
    ('Ż', "Zdotaccent"),
    ('ż', "zdotaccent"),
];

fn encode_line(s: &str) -> Vec<u8> {
    let mut out = vec![b'('];
    for ch in s.chars() {
        let code = if let Some(i) = PL_GLYPHS.iter().position(|(c, _)| *c == ch) {
            128 + i as u32
        } else if (ch as u32) < 0x80 || (0xA0..=0xFF).contains(&(ch as u32)) {
            ch as u32
        } else {
            b'?' as u32
        };
        match code {
            0x28 | 0x29 | 0x5C => {
                out.push(b'\\');
                out.push(code as u8);
            }
            0x20..=0x7E => out.push(code as u8),
            _ => out.extend_from_slice(format!("\\{code:03o}").as_bytes()),
        }
    }
    out.push(b')');
    out
}

fn font_dict(base: &str) -> Vec<u8> {
    let diffs: String = PL_GLYPHS.iter().map(|(_, g)| format!(" /{g}")).collect();
    format!("<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding << /Type /Encoding /BaseEncoding /WinAnsiEncoding /Differences [128{diffs}] >> >>")
        .into_bytes()
}

/// Obiekt XObject typu Form o podanym prostokącie, zasobach i treści.
fn form_xobject(w: f32, h: f32, resources: &str, content: &[u8]) -> Vec<u8> {
    let mut obj = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 {} {}] /Resources << {resources} >> /Length {} >>\nstream\n",
        fmt_num(w),
        fmt_num(h),
        content.len()
    )
    .into_bytes();
    obj.extend_from_slice(content);
    obj.extend_from_slice(b"\nendstream");
    obj
}

/// Część wysokości zostawiona u góry na tekst stanu, który Adobe Reader wpisuje w warstwę n4.
const STATUS_BAND: f32 = 0.28;

/// Wygląd widocznego podpisu w warstwach Acrobata (n0–n4 w formie /FRM): Adobe Reader podmienia
/// warstwę n1 na ikonę ważności (zielony znaczek, „?” albo „✖”), a n4 na tekst stanu, np.
/// „Podpis jest prawidłowy”. Inne przeglądarki pokazują tylko ramkę i tekst (n2), bo n1 i n4 są puste.
/// `ids`: [wygląd /N, FRM, n0, n1, n2, n3, n4].
fn appearance(v: &Visible, f1: ObjectId, f2: ObjectId, ids: [ObjectId; 7]) -> Vec<(ObjectId, Vec<u8>)> {
    let [ap, frm, n0, n1, n2, n3, n4] = ids;
    let w = (v.rect[2] - v.rect[0]).abs().max(10.0);
    let h = (v.rect[3] - v.rect[1]).abs().max(10.0);

    // n2: ramka i wiersze tekstu w dolnej części (nad nimi tekst stanu z Readera).
    let text_h = h * (1.0 - STATUS_BAND);
    let pad = (h * 0.06).clamp(2.0, 8.0);
    let n = v.lines.len().max(1) as f32;
    let longest = v.lines.iter().map(|l| l.chars().count()).max().unwrap_or(1).max(1) as f32;
    let size = ((text_h - pad) / (n * 1.25)).min((w - 2.0 * pad) / (longest * 0.56)).clamp(3.0, 14.0);
    let lead = size * 1.25;
    let mut s = format!("q 0.18 0.36 0.72 RG 0.75 w 0.5 0.5 {} {} re S Q\nBT 0.08 0.12 0.3 rg\n", fmt_num(w - 1.0), fmt_num(h - 1.0)).into_bytes();
    let mut y = text_h / 2.0 + n * lead / 2.0 - size;
    for (i, line) in v.lines.iter().enumerate() {
        let font = if i == 1 { "F2" } else { "F1" };
        s.extend_from_slice(format!("/{font} {} Tf 1 0 0 1 {} {} Tm ", fmt_num(size), fmt_num(pad), fmt_num(y)).as_bytes());
        s.extend(encode_line(line));
        s.extend_from_slice(b" Tj\n");
        y -= lead;
    }
    s.extend_from_slice(b"ET");
    let fonts = format!("/Font << /F1 {} {} R /F2 {} {} R >>", f1.0, f1.1, f2.0, f2.1);

    // Ikona ważności: kwadrat 100×100 przeskalowany do 90% krótszego boku, na środku.
    let side = w.min(h) * 0.9;
    let scale = side / 100.0;
    let icon = format!("{} 0 0 {} {} {} cm", fmt_num(scale), fmt_num(scale), fmt_num((w - side) / 2.0), fmt_num((h - side) / 2.0));
    // Tekst stanu (n4) tylko w górnym pasku — Reader dopasowuje go do prostokąta warstwy.
    let band = h * STATUS_BAND;
    let pad_x = (w * 0.02).max(2.0);
    let frm_content = format!(
        "q 1 0 0 1 0 0 cm /n0 Do Q\nq {icon} /n1 Do Q\nq 1 0 0 1 0 0 cm /n2 Do Q\nq {icon} /n3 Do Q\nq 1 0 0 1 {} {} cm /n4 Do Q",
        fmt_num(pad_x),
        fmt_num(h - band)
    );
    let layers = |ids: [(ObjectId, &str); 5]| -> String {
        let refs: String = ids.iter().map(|(id, name)| format!(" /{name} {} {} R", id.0, id.1)).collect();
        format!("/XObject <<{refs} >>")
    };
    vec![
        (ap, form_xobject(w, h, &format!("/XObject << /FRM {} {} R >>", frm.0, frm.1), b"q 1 0 0 1 0 0 cm /FRM Do Q")),
        (frm, form_xobject(w, h, &layers([(n0, "n0"), (n1, "n1"), (n2, "n2"), (n3, "n3"), (n4, "n4")]), frm_content.as_bytes())),
        (n0, form_xobject(100.0, 100.0, "", b"% DSBlank")),
        (n1, form_xobject(100.0, 100.0, "", b"% DSUnknown")),
        (n2, form_xobject(w, h, &fonts, &s)),
        (n3, form_xobject(100.0, 100.0, "", b"% DSBlank")),
        (n4, form_xobject(w - 2.0 * pad_x, band, "", b"% DSBlank")),
    ]
}

fn last_startxref(bytes: &[u8]) -> Option<usize> {
    let p = rfind(bytes, b"startxref")? + 9;
    let rest = &bytes[p..bytes.len().min(p + 40)];
    let digits: String = rest.iter().skip_while(|c| c.is_ascii_whitespace()).take_while(|c| c.is_ascii_digit()).map(|c| *c as char).collect();
    digits.parse().ok()
}

fn resolve_array(doc: &Document, o: Option<&Object>) -> Vec<Object> {
    match o {
        Some(Object::Array(a)) => a.clone(),
        Some(Object::Reference(r)) => doc.get_object(*r).and_then(Object::as_array).cloned().unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn resolve_dict(doc: &Document, o: Option<&Object>) -> Dictionary {
    match o {
        Some(Object::Dictionary(d)) => d.clone(),
        Some(Object::Reference(r)) => doc.get_object(*r).and_then(Object::as_dict).cloned().unwrap_or_default(),
        _ => Dictionary::new(),
    }
}

/// Aktualizacja przyrostowa PDF: nowe i zmienione obiekty dopisywane na końcu pliku,
/// oryginalne bajty zostają nietknięte (wcześniejsze podpisy pozostają ważne).
pub(crate) struct Update<'a> {
    bytes: &'a [u8],
    pub doc: Document,
    next: u32,
    objs: Vec<(ObjectId, Vec<u8>)>,
    pub root_id: ObjectId,
    /// Katalog dokumentu; zapisywany na końcu, gdy `catalog_dirty`.
    pub catalog: Dictionary,
    pub catalog_dirty: bool,
}

impl<'a> Update<'a> {
    pub fn open(bytes: &'a [u8]) -> Res<Self> {
        let doc = Document::load_mem(bytes).map_err(|e| format!("Nie można odczytać PDF: {e}"))?;
        if doc.is_encrypted() || doc.trailer.has(b"Encrypt") {
            return Err("Nie można podpisać zaszyfrowanego (chronionego hasłem) pliku PDF.".into());
        }
        let size_old = doc.trailer.get(b"Size").and_then(Object::as_i64).unwrap_or(0).max(0) as u32;
        let next = size_old.max(doc.max_id + 1);
        let root_id = doc.trailer.get(b"Root").and_then(Object::as_reference).map_err(|_| "Brak katalogu dokumentu (/Root).")?;
        let catalog = doc.get_object(root_id).and_then(Object::as_dict).map_err(|e| e.to_string())?.clone();
        Ok(Self { bytes, doc, next, objs: Vec::new(), root_id, catalog, catalog_dirty: false })
    }

    pub fn alloc(&mut self) -> ObjectId {
        self.next += 1;
        (self.next - 1, 0)
    }

    pub fn put(&mut self, id: ObjectId, body: Vec<u8>) {
        self.objs.push((id, body));
    }

    /// Nowy strumień z surowymi danymi (np. certyfikat albo CRL w DSS).
    pub fn put_stream(&mut self, data: &[u8]) -> ObjectId {
        let id = self.alloc();
        let mut body = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.put(id, body);
        id
    }

    /// Pole podpisu (widżet) na stronie `page_no` ze słownikiem podpisu `sig_dict`.
    pub fn add_signature_field(&mut self, sig_dict: Vec<u8>, field_name: &str, page_no: u32, visible: Option<&Visible>) -> Res<()> {
        let pages = self.doc.get_pages();
        let page_id = *pages.get(&page_no).ok_or("Nie ma takiej strony.")?;
        let mut page = self.doc.get_object(page_id).and_then(Object::as_dict).map_err(|e| e.to_string())?.clone();
        let sig_id = self.alloc();
        let widget_id = self.alloc();
        self.put(sig_id, sig_dict);

        let mut widget = Dictionary::new();
        widget.set("Type", name("Annot"));
        widget.set("Subtype", name("Widget"));
        widget.set("FT", name("Sig"));
        widget.set("T", text_string(&format!("{field_name}_{}", sig_id.0)));
        widget.set("V", Object::Reference(sig_id));
        widget.set("F", Object::Integer(132)); // drukowanie + zablokowana
        widget.set("P", Object::Reference(page_id));
        let rect = visible.map(|v| v.rect).unwrap_or([0.0; 4]);
        widget.set("Rect", Object::Array(rect.iter().map(|v| Object::Real(*v)).collect()));
        if let Some(v) = visible {
            let (f1, f2) = (self.alloc(), self.alloc());
            let ids: [ObjectId; 7] = std::array::from_fn(|_| self.alloc());
            self.put(f1, font_dict("Helvetica"));
            self.put(f2, font_dict("Helvetica-Bold"));
            self.objs.extend(appearance(v, f1, f2, ids));
            let mut apd = Dictionary::new();
            apd.set("N", Object::Reference(ids[0]));
            widget.set("AP", Object::Dictionary(apd));
        }
        self.put(widget_id, dict_bytes(&widget));

        let mut annots = resolve_array(&self.doc, page.get(b"Annots").ok());
        annots.push(Object::Reference(widget_id));
        page.set("Annots", Object::Array(annots));
        self.put(page_id, dict_bytes(&page));

        let add_field = |doc: &Document, form: &mut Dictionary| {
            let mut fields = resolve_array(doc, form.get(b"Fields").ok());
            fields.push(Object::Reference(widget_id));
            form.set("Fields", Object::Array(fields));
            form.set("SigFlags", Object::Integer(3));
        };
        match self.catalog.get(b"AcroForm") {
            Ok(Object::Reference(r)) => {
                let r = *r;
                let mut form = self.doc.get_object(r).and_then(Object::as_dict).cloned().unwrap_or_default();
                add_field(&self.doc, &mut form);
                self.put(r, dict_bytes(&form));
            }
            other => {
                let mut form = match other {
                    Ok(Object::Dictionary(d)) => d.clone(),
                    _ => Dictionary::new(),
                };
                add_field(&self.doc, &mut form);
                self.catalog.set("AcroForm", Object::Dictionary(form));
                self.catalog_dirty = true;
            }
        }
        Ok(())
    }

    /// Dopisuje obiekty, tablicę odsyłaczy (w formacie poprzedniej) i trailer.
    pub fn finish(mut self) -> Res<Vec<u8>> {
        let bytes = self.bytes;
        let prev = last_startxref(bytes).ok_or("Nie znaleziono tablicy odsyłaczy (xref) w pliku PDF.")?;
        let xref_stream = !bytes.get(prev..).is_some_and(|b| b.starts_with(b"xref"));
        if self.catalog_dirty {
            let body = dict_bytes(&self.catalog);
            self.put(self.root_id, body);
        }
        let mut out = bytes.to_vec();
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        let mut offsets: Vec<(ObjectId, usize)> = Vec::new();
        for (id, body) in &self.objs {
            offsets.push((*id, out.len()));
            out.extend_from_slice(format!("{} {} obj\n", id.0, id.1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let new_id = Object::String(Sha256::digest(&out)[..16].to_vec(), StringFormat::Hexadecimal);
        let ids = match self.doc.trailer.get(b"ID") {
            Ok(Object::Array(a)) if a.len() == 2 => Object::Array(vec![a[0].clone(), new_id]),
            _ => Object::Array(vec![new_id.clone(), new_id]),
        };
        let mut trailer = Dictionary::new();
        trailer.set("Root", Object::Reference(self.root_id));
        if let Ok(info) = self.doc.trailer.get(b"Info") {
            trailer.set("Info", info.clone());
        }
        trailer.set("ID", ids);
        trailer.set("Prev", Object::Integer(prev as i64));

        let startxref;
        if xref_stream {
            let xid = self.alloc();
            offsets.push((xid, out.len()));
            offsets.sort();
            let mut data = Vec::new();
            let mut index = Vec::new();
            for ((id, gen), off) in &offsets {
                data.push(1u8);
                data.extend_from_slice(&(*off as u32).to_be_bytes());
                data.extend_from_slice(&gen.to_be_bytes());
                index.extend([Object::Integer(*id as i64), Object::Integer(1)]);
            }
            trailer.set("Type", name("XRef"));
            trailer.set("Size", Object::Integer(self.next as i64));
            trailer.set("W", Object::Array(vec![Object::Integer(1), Object::Integer(4), Object::Integer(2)]));
            trailer.set("Index", Object::Array(index));
            trailer.set("Length", Object::Integer(data.len() as i64));
            startxref = out.len();
            out.extend_from_slice(format!("{} 0 obj\n", xid.0).as_bytes());
            ser_dict(&trailer, &mut out);
            out.extend_from_slice(b"\nstream\n");
            out.extend(data);
            out.extend_from_slice(b"\nendstream\nendobj\n");
        } else {
            offsets.sort();
            startxref = out.len();
            out.extend_from_slice(b"xref\n");
            let mut i = 0;
            while i < offsets.len() {
                let mut j = i + 1;
                while j < offsets.len() && offsets[j].0 .0 == offsets[j - 1].0 .0 + 1 {
                    j += 1;
                }
                out.extend_from_slice(format!("{} {}\n", offsets[i].0 .0, j - i).as_bytes());
                for ((_, gen), off) in &offsets[i..j] {
                    out.extend_from_slice(format!("{off:010} {gen:05} n\r\n").as_bytes());
                }
                i = j;
            }
            trailer.set("Size", Object::Integer(self.next as i64));
            out.extend_from_slice(b"trailer\n");
            ser_dict(&trailer, &mut out);
            out.push(b'\n');
        }
        out.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());
        Ok(out)
    }
}

/// Słownik podpisu z miejscem na /ByteRange i /Contents (`extra` — dalsze klucze, np. /M, /Reason).
fn sig_dict(kind: &[u8], sub_filter: &[u8], extra: &[u8]) -> Vec<u8> {
    let mut s = b"<< /Type /".to_vec();
    s.extend_from_slice(kind);
    s.extend_from_slice(b" /Filter /Adobe.PPKLite /SubFilter /");
    s.extend_from_slice(sub_filter);
    s.extend_from_slice(b" /ByteRange ");
    s.extend_from_slice(BR_PLACEHOLDER);
    s.extend_from_slice(b" /Contents <");
    s.extend(std::iter::repeat_n(b'0', CONTENTS_BYTES * 2));
    s.extend_from_slice(b">");
    s.extend_from_slice(extra);
    s.extend_from_slice(b" /Prop_Build << /App << /Name /PDF#20Workbench >> >> >>");
    s
}

/// Uzupełnia /ByteRange i wpisuje do /Contents wynik `make(podpisane bajty)`.
/// `base` — długość pliku przed aktualizacją (miejsce na podpis jest tylko w dopisanej części).
fn seal(mut out: Vec<u8>, base: usize, make: impl FnOnce(&[u8]) -> Res<Vec<u8>>) -> Res<Vec<u8>> {
    let lt = find(&out[base..], b"/Contents <0000").ok_or("Błąd przygotowania podpisu.")? + base + 10;
    let gt = lt + 2 + CONTENTS_BYTES * 2;
    if out.get(gt - 1) != Some(&b'>') {
        return Err("Błąd przygotowania podpisu.".into());
    }
    let p = find(&out[base..], BR_PLACEHOLDER).ok_or("Błąd przygotowania podpisu.")? + base;
    let mut br = format!("[0 {lt} {gt} {}]", out.len() - gt).into_bytes();
    br.resize(BR_PLACEHOLDER.len(), b' ');
    out[p..p + br.len()].copy_from_slice(&br);
    let data = [&out[..lt], &out[gt..]].concat();
    let contents = make(&data)?;
    let hexed = hex::encode_upper(&contents);
    if hexed.len() > CONTENTS_BYTES * 2 {
        return Err("Podpis nie mieści się w zarezerwowanym miejscu.".into());
    }
    out[lt + 1..lt + 1 + hexed.len()].copy_from_slice(hexed.as_bytes());
    Ok(out)
}

fn sign_with(bytes: &[u8], o: &SignOptions, cert: &Cert, key: &Key, chain: &[Vec<u8>]) -> Res<Vec<u8>> {
    let mut extra = b" /M ".to_vec();
    ser(&text_string(&o.signing_time), &mut extra);
    extra.extend_from_slice(b" /Name ");
    ser(&text_string(&cert.name(false)), &mut extra);
    for (key, val) in [(&b" /Reason "[..], &o.reason), (b" /Location ", &o.location)] {
        if let Some(v) = val.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            extra.extend_from_slice(key);
            ser(&text_string(v), &mut extra);
        }
    }
    let mut up = Update::open(bytes)?;
    let digits: String = o.signing_time.chars().filter(char::is_ascii_digit).take(14).collect();
    let page = o.visible.as_ref().map(|v| v.page).unwrap_or(1);
    up.add_signature_field(sig_dict(b"Sig", b"ETSI.CAdES.detached", &extra), &format!("Podpis_{digits}"), page, o.visible.as_ref())?;
    let out = up.finish()?;
    let tsa = o.timestamp.then_some(DEFAULT_TSA);
    seal(out, bytes.len(), |data| cms_sign(data, cert, key, chain, tsa))
}

/// Znacznik czasu dokumentu (PAdES-LTA): niewidoczne pole z tokenem RFC 3161 nad całym plikiem.
pub(crate) fn add_document_timestamp(bytes: &[u8], url: &str) -> Res<Vec<u8>> {
    let mut up = Update::open(bytes)?;
    up.add_signature_field(sig_dict(b"DocTimeStamp", b"ETSI.RFC3161", b""), "Znacznik_czasu", 1, None)?;
    let out = up.finish()?;
    seal(out, bytes.len(), |data| timestamp(url, data))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignReport {
    /// Osiągnięty poziom PAdES: „B-B”, „B-T” albo „B-LTA”.
    pub level: &'static str,
    /// Ostrzeżenie, gdy nie udało się dodać danych długoterminowej weryfikacji.
    pub warning: Option<String>,
}

/// Podpis + opcjonalnie dane do długoterminowej weryfikacji (DSS) i znacznik czasu dokumentu.
fn sign_full(bytes: &[u8], o: &SignOptions, cert: &Cert, key: &Key, chain: &[Vec<u8>]) -> Res<(Vec<u8>, SignReport)> {
    let signed = sign_with(bytes, o, cert, key, chain)?;
    let base_level = if o.timestamp { "B-T" } else { "B-B" };
    if !(o.ltv && o.timestamp) {
        return Ok((signed, SignReport { level: base_level, warning: None }));
    }
    match ltv::add_ltv(&signed).and_then(|with_dss| add_document_timestamp(&with_dss, DEFAULT_TSA)) {
        Ok(out) => Ok((out, SignReport { level: "B-LTA", warning: None })),
        Err(e) => Ok((
            signed,
            SignReport { level: base_level, warning: Some(format!("Podpisano, ale bez danych do długoterminowej weryfikacji: {e}")) },
        )),
    }
}

/// Podpisuje PDF certyfikatem z magazynu Windows (PAdES, opcjonalnie ze znacznikiem czasu i LTA).
pub fn sign_pdf(bytes: &[u8], o: &SignOptions) -> Res<(Vec<u8>, SignReport)> {
    let cert = find_cert(&o.cert_id)?;
    let key = acquire_key(&cert)?;
    let chain = chain_certs(&cert);
    sign_full(bytes, o, &cert, &key, &chain)
}

/// Odpowiedź [u32 LE długość JSON][JSON][PDF].
pub fn pack<T: Serialize>(pdf: Vec<u8>, report: &T) -> Vec<u8> {
    let json = serde_json::to_vec(report).unwrap_or_default();
    let mut out = (json.len() as u32).to_le_bytes().to_vec();
    out.extend(json);
    out.extend(pdf);
    out
}

/// Strona PDF: prostokąt MediaBox (z dziedziczeniem po drzewie stron).
fn media_box(doc: &Document, page_id: ObjectId) -> [f32; 4] {
    let mut id = Some(page_id);
    for _ in 0..32 {
        let Some(cur) = id else { break };
        let Ok(d) = doc.get_object(cur).and_then(Object::as_dict) else { break };
        if let Ok(arr) = d.get(b"MediaBox").and_then(|o| match o {
            Object::Reference(r) => doc.get_object(*r),
            o => Ok(o),
        }).and_then(Object::as_array)
        {
            let v: Vec<f32> = arr.iter().filter_map(|x| x.as_float().ok().or_else(|| x.as_i64().ok().map(|i| i as f32))).collect();
            if v.len() == 4 {
                return [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])];
            }
        }
        id = d.get(b"Parent").and_then(Object::as_reference).ok();
    }
    [0.0, 0.0, 595.0, 842.0]
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchOptions {
    pub paths: Vec<String>,
    pub sign: SignOptions,
    /// Wiersze widocznego podpisu; brak — podpis niewidoczny.
    pub visible_lines: Option<Vec<String>>,
    /// Miejsce widocznego podpisu dla kolejnych plików (jak `paths`); brak — prawy dolny róg
    /// ostatniej strony. Strona dalej niż koniec dokumentu oznacza ostatnią.
    #[serde(default)]
    pub placements: Vec<Option<Placement>>,
}

#[derive(Deserialize, Clone)]
pub struct Placement {
    pub page: u32,
    pub rect: [f32; 4],
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatchItem {
    pub path: String,
    pub output: Option<String>,
    pub level: Option<&'static str>,
    pub warning: Option<String>,
    pub error: Option<String>,
}

fn output_path(path: &str) -> String {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "dokument".into());
    let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{stem} (podpisany).pdf") } else { format!("{stem} (podpisany {n}).pdf") };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
        n += 1;
    }
}

/// Podpisuje wiele plików jednym certyfikatem (klucz pobierany raz). Wynik każdego pliku
/// trafia do `progress` zaraz po podpisaniu; podpisane kopie obok oryginałów.
pub fn sign_files(o: &BatchOptions, progress: impl FnMut(usize, &BatchItem)) -> Res<Vec<BatchItem>> {
    let cert = find_cert(&o.sign.cert_id)?;
    let key = acquire_key(&cert)?;
    let chain = chain_certs(&cert);
    sign_files_with(o, &cert, &key, &chain, progress)
}

fn sign_files_with(o: &BatchOptions, cert: &Cert, key: &Key, chain: &[Vec<u8>], mut progress: impl FnMut(usize, &BatchItem)) -> Res<Vec<BatchItem>> {
    let mut results = Vec::new();
    for (i, path) in o.paths.iter().enumerate() {
        let item = (|| -> Res<BatchItem> {
            let bytes = std::fs::read(path).map_err(|e| format!("Nie można odczytać pliku: {e}"))?;
            let mut opts = SignOptions { visible: None, ..o.sign.clone() };
            if let Some(lines) = &o.visible_lines {
                let doc = Document::load_mem(&bytes).map_err(|e| format!("Nie można odczytać PDF: {e}"))?;
                let pages = doc.get_pages();
                let (&last, &page_id) = pages.iter().next_back().ok_or("Dokument nie ma stron.")?;
                let (page, rect) = match o.placements.get(i).cloned().flatten() {
                    Some(p) => (p.page.clamp(1, last), p.rect),
                    None => {
                        let mb = media_box(&doc, page_id);
                        let (w, h) = (220.0_f32.min((mb[2] - mb[0]) * 0.45), 62.0);
                        (last, [mb[2] - 36.0 - w, mb[1] + 36.0, mb[2] - 36.0, mb[1] + 36.0 + h])
                    }
                };
                opts.visible = Some(Visible { page, rect, lines: lines.clone() });
            }
            let (out, report) = sign_full(&bytes, &opts, cert, key, chain)?;
            let target = output_path(path);
            std::fs::write(&target, &out).map_err(|e| format!("Nie można zapisać pliku: {e}"))?;
            Ok(BatchItem { path: path.clone(), output: Some(target), level: Some(report.level), warning: report.warning, error: None })
        })()
        .unwrap_or_else(|e| BatchItem { path: path.clone(), output: None, level: None, warning: None, error: Some(e) });
        progress(i, &item);
        results.push(item);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Klucz RSA tylko w pamięci i certyfikat z podpisem własnym — bez zmian w magazynie systemu.
    fn ephemeral() -> (Cert, Key) {
        unsafe {
            let mut prov = NCRYPT_PROV_HANDLE(0);
            NCryptOpenStorageProvider(&mut prov, MS_KEY_STORAGE_PROVIDER, 0).unwrap();
            let mut key = NCRYPT_KEY_HANDLE(0);
            NCryptCreatePersistedKey(prov, &mut key, NCRYPT_RSA_ALGORITHM, PCWSTR::null(), CERT_KEY_SPEC(0), NCRYPT_FLAGS(0)).unwrap();
            NCryptSetProperty(NCRYPT_HANDLE(key.0), NCRYPT_LENGTH_PROPERTY, &2048u32.to_le_bytes(), NCRYPT_FLAGS(0)).unwrap();
            NCryptFinalizeKey(key, NCRYPT_FLAGS(0)).unwrap();
            let x500 = w!("CN=Paweł Żółćtest, O=PDF Workbench Test");
            let mut n = 0u32;
            CertStrToNameW(X509_ASN_ENCODING, x500, CERT_X500_NAME_STR, None, None, &mut n, None).unwrap();
            let mut buf = vec![0u8; n as usize];
            CertStrToNameW(X509_ASN_ENCODING, x500, CERT_X500_NAME_STR, None, Some(buf.as_mut_ptr()), &mut n, None).unwrap();
            let subject = CRYPT_INTEGER_BLOB { cbData: n, pbData: buf.as_mut_ptr() };
            let alg = CRYPT_ALGORITHM_IDENTIFIER { pszObjId: PSTR(b"1.2.840.113549.1.1.11\0".as_ptr() as *mut u8), Parameters: std::mem::zeroed() };
            // Jak certyfikat do podpisu: KeyUsage = digitalSignature + nonRepudiation, ważny 2020–2035.
            let mut ku = [0x03u8, 0x02, 0x06, 0xC0];
            let mut ext = CERT_EXTENSION {
                pszObjId: PSTR(b"2.5.29.15\0".as_ptr() as *mut u8),
                fCritical: true.into(),
                Value: CRYPT_INTEGER_BLOB { cbData: ku.len() as u32, pbData: ku.as_mut_ptr() },
            };
            let exts = CERT_EXTENSIONS { cExtension: 1, rgExtension: &mut ext };
            let day = |y| windows::Win32::Foundation::SYSTEMTIME { wYear: y, wMonth: 1, wDay: 1, ..Default::default() };
            let (start, end) = (day(2020), day(2035));
            let c = CertCreateSelfSignCertificate(
                Some(HCRYPTPROV_OR_NCRYPT_KEY_HANDLE(key.0)),
                &subject,
                CERT_CREATE_SELFSIGN_NO_KEY_INFO,
                None,
                Some(&alg),
                Some(&start),
                Some(&end),
                Some(&exts),
            );
            assert!(!c.is_null(), "certyfikat testowy: {}", windows::core::Error::from_thread());
            let _ = NCryptFreeObject(NCRYPT_HANDLE(prov.0));
            (Cert(c), Key { handle: HCRYPTPROV_OR_NCRYPT_KEY_HANDLE(key.0), spec: CERT_NCRYPT_KEY_SPEC, free: true })
        }
    }

    fn opts(page: u32, ts: bool) -> SignOptions {
        SignOptions {
            cert_id: String::new(),
            signing_time: "D:20261105120000+01'00'".into(),
            reason: Some("Akceptuję treść umowy".into()),
            location: Some("Łódź".into()),
            timestamp: ts,
            ltv: false,
            visible: Some(Visible {
                page,
                rect: [330.0, 40.0, 560.0, 100.0],
                lines: vec!["Podpisany elektronicznie przez:".into(), "Paweł Żółćtest".into(), "Data: 2026-11-06 12:00:00 +01:00".into()],
            }),
        }
    }

    #[test]
    fn sign_verify_tamper_and_cosign() {
        let (cert, key) = ephemeral();
        let ts = std::env::var("PDFWB_TEST_TSA").is_ok();
        let out_dir = std::env::var("PDFWB_TEST_OUT").ok();
        let samples = concat!(env!("CARGO_MANIFEST_DIR"), "/../samples");
        let files: Vec<_> = std::fs::read_dir(samples).unwrap().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "pdf")).collect();
        assert!(!files.is_empty());
        for path in files {
            let pdf = std::fs::read(&path).unwrap();
            assert!(inspect(&pdf).is_empty(), "{path:?} nie powinien mieć podpisów");
            let signed = sign_with(&pdf, &opts(1, ts), &cert, &key, &[]).unwrap_or_else(|e| panic!("{path:?}: {e}"));
            assert_eq!(&signed[..pdf.len()], &pdf[..], "oryginalne bajty muszą zostać nietknięte");
            Document::load_mem(&signed).unwrap_or_else(|e| panic!("{path:?}: wynik nie jest poprawnym PDF: {e}"));
            let s = inspect(&signed);
            assert_eq!(s.len(), 1);
            assert!(s[0].intact && s[0].covers_whole, "{path:?}: {:?}", s[0].problem);
            assert_eq!(s[0].signer.as_deref(), Some("Paweł Żółćtest"));
            assert_eq!(s[0].timestamped, ts);
            assert_eq!(s[0].reason.as_deref(), Some("Akceptuję treść umowy"));

            // Zmiana jednego bajtu treści unieważnia podpis.
            let mut bad = signed.clone();
            let at = pdf.len() / 2;
            bad[at] ^= 0x01;
            assert!(!inspect(&bad)[0].intact, "{path:?}: zmiana treści musi unieważnić podpis");

            // Drugi podpis: pierwszy pozostaje nienaruszony, ale nie obejmuje już całego pliku.
            let twice = sign_with(&signed, &opts(1, false), &cert, &key, &[]).unwrap();
            let s2 = inspect(&twice);
            assert_eq!(s2.len(), 2);
            assert!(s2[0].intact && !s2[0].covers_whole);
            assert!(s2[1].intact && s2[1].covers_whole);

            if let Some(dir) = &out_dir {
                let stem = path.file_stem().unwrap().to_string_lossy();
                std::fs::write(format!("{dir}/{stem}-podpisany.pdf"), &signed).unwrap();
                std::fs::write(format!("{dir}/{stem}-2podpisy.pdf"), &twice).unwrap();
                let lt = find(&signed[pdf.len()..], b"/Contents <").unwrap() + pdf.len() + 10;
                let raw = decode_hex(&signed[lt + 1..lt + 1 + CONTENTS_BYTES * 2]);
                std::fs::write(format!("{dir}/{stem}.p7s"), &raw[..der_len(&raw).unwrap()]).unwrap();
            }
        }
    }

    /// PAdES-LTA ze znacznikiem czasu Certum, CRL i unijną listą zaufanych (wymaga sieci:
    /// uruchamiany, gdy ustawiono PDFWB_TEST_TSA).
    #[test]
    fn lta_and_trust() {
        if std::env::var("PDFWB_TEST_TSA").is_err() {
            return;
        }
        let (cert, key) = ephemeral();
        let pdf = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../samples/umowa-testowa.pdf")).unwrap();
        let mut o = opts(1, true);
        o.ltv = true;
        let (out, report) = sign_full(&pdf, &o, &cert, &key, &[]).unwrap();
        assert_eq!(report.level, "B-LTA", "{:?}", report.warning);
        Document::load_mem(&out).unwrap();
        let s = inspect(&out);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, "signature");
        assert!(s[0].intact && !s[0].covers_whole && s[0].timestamped);
        assert_eq!(s[1].kind, "timestamp");
        assert!(s[1].intact && s[1].covers_whole, "{:?}", s[1].problem);
        assert!(ltv::has_ltv(&out));

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let dir = std::env::temp_dir().join("pdfwb-tsl-test");
        let http = reqwest::Client::new();
        let pl = rt.block_on(trust::tl_services(&http, &dir, "PL")).unwrap();
        let ca = pl.iter().filter(|x| x.kind == "ca").count();
        let tsa = pl.iter().filter(|x| x.kind == "tsa").count();
        eprintln!("PL TL: {ca} usług CA/QC, {tsa} TSA/QTST; np. {:?}", pl.iter().map(|x| &x.name).take(6).collect::<Vec<_>>());
        assert!(ca > 0);
        let countries = issuer_countries(&out);
        eprintln!("kraje wystawców: {countries:?}");
        let mut lists = std::collections::HashMap::new();
        lists.insert("PL".to_string(), pl);
        let t = check_trust(&out, &lists, &[]);
        for x in &t {
            eprintln!("zaufanie: chain={} {:?} rev={} eu={} {:?} czas={:?} tsq={} ltv={} {:?}", x.chain_trusted, x.chain, x.revocation, x.eu_qualified, x.eu_service, x.trusted_time, x.timestamp_qualified, x.ltv, x.notes);
        }
        assert_eq!(t.len(), 2);
        assert!(t[0].trusted_time.is_some() && !t[0].eu_qualified && t[0].ltv);
        if let Ok(dir) = std::env::var("PDFWB_TEST_OUT") {
            std::fs::write(format!("{dir}/umowa-lta.pdf"), &out).unwrap();
        }
    }

    #[test]
    fn batch_signs_copies_next_to_originals() {
        let (cert, key) = ephemeral();
        let dir = std::env::temp_dir().join(format!("pdfwb-batch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let samples = concat!(env!("CARGO_MANIFEST_DIR"), "/../samples");
        let mut paths = Vec::new();
        for name in ["umowa-testowa.pdf", "skan-testowy.pdf"] {
            let target = dir.join(name);
            std::fs::copy(format!("{samples}/{name}"), &target).unwrap();
            paths.push(target.to_string_lossy().to_string());
        }
        paths.push(dir.join("brak.pdf").to_string_lossy().to_string());
        let o = BatchOptions {
            paths,
            sign: SignOptions { visible: None, ..opts(1, false) },
            visible_lines: Some(vec!["Podpisany elektronicznie przez:".into(), "Test".into()]),
            placements: vec![None, Some(Placement { page: 99, rect: [50.0, 50.0, 250.0, 110.0] })],
        };
        let mut seen = Vec::new();
        let r = sign_files_with(&o, &cert, &key, &[], |i, _| seen.push(i)).unwrap();
        assert_eq!(seen, vec![0, 1, 2]);
        assert!(r[2].error.is_some());
        for it in &r[..2] {
            let out = it.output.as_ref().expect("plik wynikowy");
            assert!(out.ends_with(" (podpisany).pdf"), "{out}");
            let bytes = std::fs::read(out).unwrap();
            let s = inspect(&bytes);
            assert!(s.len() == 1 && s[0].intact && s[0].covers_whole);
            // Widoczny podpis na ostatniej stronie.
            let doc = Document::load_mem(&bytes).unwrap();
            let last = *doc.get_pages().iter().next_back().unwrap().1;
            let page = doc.get_object(last).unwrap().as_dict().unwrap();
            assert!(page.get(b"Annots").is_ok());
            if out.contains("skan-testowy") {
                // Wskazane miejsce (strona 99 → ostatnia): lewy dolny róg.
                assert!(find(&bytes[bytes.len() - 60_000..], b"/Rect [50 50 250 110]").is_some());
            }
        }
        // Kolejny podpis tego samego pliku nie nadpisuje poprzedniej kopii.
        let again = sign_files_with(&BatchOptions { paths: o.paths[..1].to_vec(), ..o }, &cert, &key, &[], |_, _| {}).unwrap();
        assert!(again[0].output.as_ref().unwrap().ends_with(" (podpisany 2).pdf"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Warianty do ręcznego sprawdzenia w Acrobacie (PDFWB_TEST_VARIANTS=katalog).
    #[test]
    fn variants_for_acrobat() {
        let Ok(dir) = std::env::var("PDFWB_TEST_VARIANTS") else { return };
        let (cert, key) = ephemeral();
        let pdf = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../samples/umowa-testowa.pdf")).unwrap();
        let signed = sign_with(&pdf, &opts(1, true), &cert, &key, &[]).unwrap();
        std::fs::write(format!("{dir}/v-bt.pdf"), &signed).unwrap();
        std::fs::write(format!("{dir}/v-doc-ts.pdf"), add_document_timestamp(&signed, DEFAULT_TSA).unwrap()).unwrap();
        std::fs::write(format!("{dir}/v-dss.pdf"), ltv::add_ltv(&signed).unwrap()).unwrap();
    }

    /// Pełny PAdES-LTA z realistycznym łańcuchem (testowe CA + certyfikat z CRL) z pliku PFX
    /// importowanego tylko do pamięci. PDFWB_TEST_PFX=katalog z leaf.pfx i ca.der.
    #[test]
    fn lta_with_test_ca() {
        let Ok(dir) = std::env::var("PDFWB_TEST_PFX") else { return };
        unsafe {
            let mut pfx = std::fs::read(format!("{dir}/leaf.pfx")).unwrap();
            let blob = CRYPT_INTEGER_BLOB { cbData: pfx.len() as u32, pbData: pfx.as_mut_ptr() };
            let store = Store(PFXImportCertStore(&blob, w!("test"), PKCS12_NO_PERSIST_KEY | PKCS12_ALWAYS_CNG_KSP).unwrap());
            let mut prev: *mut CERT_CONTEXT = null_mut();
            let mut leaf = None;
            loop {
                prev = CertEnumCertificatesInStore(store.0, if prev.is_null() { None } else { Some(prev) });
                if prev.is_null() {
                    break;
                }
                let c = Cert(CertDuplicateCertificateContext(Some(prev)));
                if c.has_private_key() {
                    leaf = Some(c);
                }
            }
            let cert = leaf.expect("certyfikat z kluczem");
            // Klucz nieutrwalony: uchwyt CNG jest we właściwości certyfikatu.
            let mut h = 0usize;
            let mut n = std::mem::size_of::<usize>() as u32;
            CertGetCertificateContextProperty(cert.0, CERT_NCRYPT_KEY_HANDLE_PROP_ID, Some(&mut h as *mut usize as *mut c_void), &mut n).unwrap();
            let key = Key { handle: HCRYPTPROV_OR_NCRYPT_KEY_HANDLE(h), spec: CERT_NCRYPT_KEY_SPEC, free: false };
            let ca = std::fs::read(format!("{dir}/ca.der")).unwrap();
            let pdf = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../samples/umowa-testowa.pdf")).unwrap();
            let mut o = opts(1, true);
            o.ltv = true;
            o.visible.as_mut().unwrap().lines[1] = cert.name(false);
            let (out, report) = sign_full(&pdf, &o, &cert, &key, &[ca]).unwrap();
            assert_eq!(report.level, "B-LTA", "{:?}", report.warning);
            std::fs::write(format!("{dir}/lta-ca.pdf"), &out).unwrap();
            let s = inspect(&out);
            assert!(s.iter().all(|x| x.intact));
        }
    }
}
