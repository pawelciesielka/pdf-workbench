//! Zaufanie do podpisów: łańcuch certyfikatów Windows, unieważnienie, zaufany czas ze znacznika
//! czasu oraz unijna lista zaufanych (EUTL): wystawca podpisu kwalifikowanego musi być usługą
//! „CA/QC” o statusie „granted” na liście zaufanych kraju wystawcy.

use super::ltv::{chain_of, cms_signer, embedded_timestamp, has_ltv, self_signed, signature_value};
use super::*;
use std::collections::HashMap;

const LOTL_URL: &str = "https://ec.europa.eu/tools/lotl/eu-lotl.xml";
/// Jak długo listy zaufanych z pamięci podręcznej są aktualne.
const TL_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(3 * 24 * 3600);

/// Usługa zaufania z listy krajowej.
#[derive(Clone, Debug)]
pub struct TlService {
    pub name: String,
    /// "ca" — wystawca certyfikatów kwalifikowanych, "tsa" — kwalifikowane znaczniki czasu.
    pub kind: &'static str,
    pub certs: Vec<Vec<u8>>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TrustInfo {
    /// Łańcuch prowadzi do certyfikatu głównego zaufanego w Windows.
    pub chain_trusted: bool,
    pub chain: Vec<String>,
    /// "good", "revoked" albo "unknown".
    pub revocation: &'static str,
    /// Wystawca jest kwalifikowanym dostawcą z unijnej listy zaufanych, a certyfikat — kwalifikowany.
    pub eu_qualified: bool,
    pub eu_service: Option<String>,
    /// Czas podpisu ze znacznika czasu (ms od 1970), jeśli jest.
    pub trusted_time: Option<i64>,
    /// Znacznik czasu pochodzi od kwalifikowanego dostawcy z listy zaufanych.
    pub timestamp_qualified: bool,
    /// W pliku są dane do długoterminowej weryfikacji (DSS).
    pub ltv: bool,
    pub notes: Vec<String>,
}

fn country_of(cert: &Cert, issuer: bool) -> Option<String> {
    unsafe {
        let flags = if issuer { CERT_NAME_ISSUER_FLAG } else { 0 };
        let oid = b"2.5.4.6\0";
        let para = Some(oid.as_ptr() as *const c_void);
        let n = CertGetNameStringW(cert.0, CERT_NAME_ATTR_TYPE, flags, para, None);
        let mut buf = vec![0u16; n.max(1) as usize];
        CertGetNameStringW(cert.0, CERT_NAME_ATTR_TYPE, flags, para, Some(&mut buf));
        let s = String::from_utf16_lossy(&buf).trim_end_matches('\0').trim().to_uppercase();
        (s.len() == 2).then_some(s)
    }
}

/// Kraje wystawców podpisów i znaczników czasu w pliku — dla nich pobierane są listy zaufanych.
pub fn issuer_countries(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    for r in raw_signatures(bytes) {
        let mut tokens = vec![r.cms.clone()];
        if let Some(t) = embedded_timestamp(&r.cms) {
            tokens.push(t.to_vec());
        }
        for t in tokens {
            if let Some((_s, c)) = cms_signer(&t) {
                if let Some(cc) = country_of(&c, true) {
                    if !out.contains(&cc) {
                        out.push(cc);
                    }
                }
            }
        }
    }
    out
}

// --- Listy zaufanych (XML ETSI TS 119 612) -------------------------------------------------------

/// Znaczniki otwierające elementu o nazwie lokalnej `tag` (z dowolnym prefiksem i atrybutami):
/// (pełna nazwa z prefiksem, indeks za „>”).
fn open_tags<'a>(xml: &'a str, tag: &str) -> Vec<(&'a str, usize)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(p) = xml[from..].find('<') {
        let lt = from + p;
        from = lt + 1;
        let rest = &xml[lt + 1..];
        // Znacznik zamykający („</…”) daje pustą nazwę i jest pomijany.
        let end_name = rest.find(|c: char| c == '>' || c == '/' || c.is_whitespace()).unwrap_or(rest.len());
        let qname = &rest[..end_name];
        let local = qname.rsplit(':').next().unwrap_or(qname);
        if local != tag {
            continue;
        }
        if let Some(gt) = rest.find('>') {
            if !rest[..gt].ends_with('/') {
                out.push((qname, lt + 1 + gt + 1));
            }
        }
    }
    out
}

/// Tekst pierwszego elementu o nazwie lokalnej `tag`.
fn tag_text<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let (_, start) = *open_tags(xml, tag).first()?;
    let rest = &xml[start..];
    Some(rest[..rest.find('<').unwrap_or(rest.len())].trim())
}

/// Zawartość kolejnych elementów `tag` (bez zagnieżdżeń tego samego elementu).
fn blocks<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut skip_to = 0;
    for (qname, start) in open_tags(xml, tag) {
        if start < skip_to {
            continue;
        }
        let end_tag = format!("</{qname}>");
        if let Some(e) = xml[start..].find(&end_tag) {
            out.push(&xml[start..start + e]);
            skip_to = start + e + end_tag.len();
        }
    }
    out
}

/// Adres listy zaufanych kraju `cc` z listy list (LOTL).
fn tl_location(lotl: &str, cc: &str) -> Option<String> {
    blocks(lotl, "OtherTSLPointer").into_iter().find_map(|b| {
        let territory = tag_text(b, "SchemeTerritory")?;
        let mime = tag_text(b, "MimeType").unwrap_or("");
        let loc = tag_text(b, "TSLLocation")?;
        (territory.eq_ignore_ascii_case(cc) && (mime.contains("xml") || loc.to_lowercase().ends_with(".xml"))).then(|| loc.to_string())
    })
}

/// Usługi CA/QC i TSA/QTST o bieżącym statusie „granted” (lub dawnym „accredited”/„undersupervision”).
pub fn parse_tl(xml: &str) -> Vec<TlService> {
    let mut out = Vec::new();
    for provider in blocks(xml, "TrustServiceProvider") {
        let tsp = blocks(provider, "TSPName").first().and_then(|n| tag_text(n, "Name")).unwrap_or("").to_string();
        out.extend(parse_services(provider, &tsp));
    }
    out
}

fn parse_services(xml: &str, tsp: &str) -> Vec<TlService> {
    use base64::Engine;
    let mut out = Vec::new();
    for b in blocks(xml, "TSPService") {
        // Bieżące dane usługi są przed historią.
        let cur = &b[..b.find("ServiceHistory").unwrap_or(b.len())];
        let typ = tag_text(cur, "ServiceTypeIdentifier").unwrap_or("");
        let status = tag_text(cur, "ServiceStatus").unwrap_or("");
        let kind = if typ.ends_with("/Svctype/CA/QC") {
            "ca"
        } else if typ.ends_with("/Svctype/TSA/QTST") {
            "tsa"
        } else {
            continue;
        };
        if !(status.ends_with("/granted") || status.ends_with("/accredited") || status.ends_with("/undersupervision")) {
            continue;
        }
        let service = tag_text(cur, "Name").unwrap_or("");
        let name = if tsp.is_empty() { service.to_string() } else { format!("{tsp} — {service}") };
        let certs: Vec<Vec<u8>> = blocks(cur, "X509Certificate")
            .into_iter()
            .filter_map(|c| {
                let clean: String = c.chars().filter(|ch| !ch.is_whitespace()).collect();
                base64::engine::general_purpose::STANDARD.decode(clean).ok()
            })
            .collect();
        if !certs.is_empty() {
            out.push(TlService { name, kind, certs });
        }
    }
    out
}

async fn cached_get(http: &reqwest::Client, url: &str, file: &std::path::Path) -> Res<String> {
    if let Ok(meta) = std::fs::metadata(file) {
        if meta.modified().ok().and_then(|m| m.elapsed().ok()).is_some_and(|age| age < TL_MAX_AGE) {
            if let Ok(s) = std::fs::read_to_string(file) {
                return Ok(s);
            }
        }
    }
    let fetched = async {
        let r = http.get(url).timeout(std::time::Duration::from_secs(60)).send().await.map_err(|e| e.to_string())?;
        if !r.status().is_success() {
            return Err(format!("HTTP {}", r.status()));
        }
        r.text().await.map_err(|e| e.to_string())
    }
    .await;
    match fetched {
        Ok(text) => {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(file, &text);
            Ok(text)
        }
        // Bez sieci: starsza kopia jest lepsza niż żadna.
        Err(e) => std::fs::read_to_string(file).map_err(|_| format!("nie udało się pobrać {url}: {e}")),
    }
}

/// Usługi z listy zaufanych kraju `cc` (z pamięcią podręczną w `dir`).
pub async fn tl_services(http: &reqwest::Client, dir: &std::path::Path, cc: &str) -> Res<Vec<TlService>> {
    let lotl = cached_get(http, LOTL_URL, &dir.join("eu-lotl.xml")).await?;
    let url = tl_location(&lotl, cc).ok_or_else(|| format!("brak listy zaufanych dla kraju {cc}"))?;
    let tl = cached_get(http, &url, &dir.join(format!("{cc}.xml"))).await?;
    Ok(parse_tl(&tl))
}

// --- Ocena podpisów -------------------------------------------------------------------------------

fn ms_to_ft(ms: i64) -> FILETIME {
    let v = (ms * 10_000 + 116_444_736_000_000_000) as u64;
    FILETIME { dwLowDateTime: v as u32, dwHighDateTime: (v >> 32) as u32 }
}

/// Czas ze znacznika czasu (po sprawdzeniu, że dotyczy `data`) i jego wystawca.
fn timestamp_time(token: &[u8], data: &[u8]) -> Option<(i64, Cert)> {
    unsafe {
        let mut ctx: *mut CRYPT_TIMESTAMP_CONTEXT = null_mut();
        let mut signer: *mut CERT_CONTEXT = null_mut();
        let mut store = HCERTSTORE(null_mut());
        let ok = CryptVerifyTimeStampSignature(token, Some(data), None, &mut ctx, &mut signer, Some(&mut store)).is_ok();
        let time = (!ctx.is_null()).then(|| ft_ms(&std::ptr::addr_of!((*(*ctx).pTimeStamp).ftTime).read_unaligned()));
        if !ctx.is_null() {
            CryptMemFree(Some(ctx as *const c_void));
        }
        if !store.0.is_null() {
            let _ = CertCloseStore(Some(store), 0);
        }
        let cert = (!signer.is_null()).then(|| Cert(signer));
        if ok {
            Some((time?, cert?))
        } else {
            None
        }
    }
}

/// Usługa z listy zaufanych, która wystawiła `cert` (zgodna nazwa i poprawny podpis certyfikatu).
fn tl_issuer(cert: &Cert, services: &[TlService], kind: &str) -> Option<(String, Cert)> {
    unsafe {
        for s in services.iter().filter(|s| s.kind == kind) {
            for der in &s.certs {
                let c = CertCreateCertificateContext(ENC, der);
                if c.is_null() {
                    continue;
                }
                let ca = Cert(c);
                let same_name = CertCompareCertificateName(ENC, &cert.info().Issuer, &ca.info().Subject).as_bool();
                let signed_by = CryptVerifyCertificateSignatureEx(None, ENC, 2, cert.0 as *const c_void, 2, Some(ca.0 as *const c_void), CRYPT_VERIFY_CERT_FLAGS(0), None).is_ok();
                // Usługa TSA może być wpisana certyfikatem samego serwera znaczników czasu.
                let is_itself = cert.der() == ca.der();
                if (same_name && signed_by) || is_itself {
                    return Some((s.name.clone(), ca));
                }
            }
        }
    }
    None
}

/// Sprawdzenie unieważnienia certyfikatu przy znanym wystawcy (CRL/OCSP pobiera Windows).
fn revocation_with_issuer(cert: &Cert, issuer: &Cert) -> &'static str {
    unsafe {
        let mut para: CERT_REVOCATION_PARA = std::mem::zeroed();
        para.cbSize = std::mem::size_of::<CERT_REVOCATION_PARA>() as u32;
        para.pIssuerCert = issuer.0;
        let mut status: CERT_REVOCATION_STATUS = std::mem::zeroed();
        status.cbSize = std::mem::size_of::<CERT_REVOCATION_STATUS>() as u32;
        let ctx = [cert.0 as *const c_void];
        match CertVerifyRevocation(ENC.0, CERT_CONTEXT_REVOCATION_TYPE, &ctx, 0, Some(&para), &mut status) {
            Ok(()) => "good",
            Err(_) if status.dwError == 0x8009_2010 => "revoked", // CRYPT_E_REVOKED
            Err(_) => "unknown",
        }
    }
}

/// Ocena zaufania każdego podpisu (w kolejności `inspect`). `lists` — usługi z list zaufanych
/// według kraju; `list_errors` — kraje, których listy nie udało się pobrać.
pub fn check_trust(bytes: &[u8], lists: &HashMap<String, Vec<TlService>>, list_errors: &[String]) -> Vec<TrustInfo> {
    let ltv = has_ltv(bytes);
    raw_signatures(bytes)
        .into_iter()
        .map(|r| {
            let mut t = TrustInfo { ltv, revocation: "unknown", ..Default::default() };
            t.notes.extend(list_errors.iter().cloned());
            let Some((store, signer)) = cms_signer(&r.cms) else {
                t.notes.push("Nie można odczytać certyfikatu podpisującego.".into());
                return t;
            };

            // Zaufany czas: znacznik czasu podpisu (nad wartością podpisu) albo sam znacznik dokumentu.
            let ts = if r.doc_timestamp {
                timestamp_time(&r.cms, &r.data)
            } else {
                embedded_timestamp(&r.cms).zip(signature_value(&r.cms)).and_then(|(tok, sig)| timestamp_time(tok, &sig))
            };
            if let Some((ms, tsa)) = &ts {
                t.trusted_time = Some(*ms);
                if let Some(cc) = country_of(tsa, true) {
                    if let Some(services) = lists.get(&cc) {
                        t.timestamp_qualified = tl_issuer(tsa, services, "tsa").is_some() || tl_issuer(tsa, services, "ca").is_some();
                    }
                }
            }
            let at = t.trusted_time.map(ms_to_ft);

            // Łańcuch Windows (z certyfikatami z podpisu) z kontrolą unieważnienia.
            let (chain, status) = chain_of(&signer, Some(store.0), at, CERT_CHAIN_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT);
            t.chain = chain.iter().map(|c| c.name(false)).collect();
            let revocation_flags = CERT_TRUST_REVOCATION_STATUS_UNKNOWN | CERT_TRUST_IS_OFFLINE_REVOCATION;
            t.chain_trusted = status != u32::MAX && status & !revocation_flags == 0;
            t.revocation = if status != u32::MAX && status & CERT_TRUST_IS_REVOKED != 0 {
                "revoked"
            } else if status != u32::MAX && status & revocation_flags == 0 {
                "good"
            } else {
                "unknown"
            };
            if status != u32::MAX && status & CERT_TRUST_IS_NOT_TIME_VALID != 0 {
                t.notes.push("Certyfikat nie był ważny w chwili podpisu.".into());
            }

            // Unijna lista zaufanych.
            if !r.doc_timestamp {
                let kind_ok = signer.qualified();
                if let Some(cc) = country_of(&signer, true) {
                    match lists.get(&cc) {
                        Some(services) => {
                            if let Some((service, ca)) = tl_issuer(&signer, services, "ca") {
                                t.eu_service = Some(service);
                                t.eu_qualified = kind_ok;
                                if t.revocation == "unknown" {
                                    t.revocation = revocation_with_issuer(&signer, &ca);
                                }
                            }
                        }
                        None if list_errors.is_empty() => t.notes.push(format!("Brak listy zaufanych dla kraju {cc}.")),
                        None => {}
                    }
                }
                if self_signed(&signer) {
                    t.notes.push("Certyfikat z podpisem własnym — tożsamości podpisującego nie da się potwierdzić.".into());
                }
            }
            t
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trusted_list_fragments() {
        let lotl = r#"<tsl:PointersToOtherTSL><tsl:OtherTSLPointer><tsl:TSLLocation>https://example.pl/PL_TSL.xml</tsl:TSLLocation>
            <tsl:AdditionalInformation><tsl:OtherInformation><tsl:SchemeTerritory>PL</tsl:SchemeTerritory></tsl:OtherInformation>
            <tsl:OtherInformation><ns3:MimeType>application/vnd.etsi.tsl+xml</ns3:MimeType></tsl:OtherInformation></tsl:AdditionalInformation>
            </tsl:OtherTSLPointer></tsl:PointersToOtherTSL>"#;
        assert_eq!(tl_location(lotl, "PL").as_deref(), Some("https://example.pl/PL_TSL.xml"));
        let tl = r#"<TSPService><ServiceInformation><ServiceTypeIdentifier>http://uri.etsi.org/TrstSvc/Svctype/CA/QC</ServiceTypeIdentifier>
            <ServiceName><Name xml:lang="en">Certum QCA 2017</Name></ServiceName>
            <ServiceDigitalIdentity><DigitalId><X509Certificate>AQID</X509Certificate></DigitalId></ServiceDigitalIdentity>
            <ServiceStatus>http://uri.etsi.org/TrstSvc/TrustedList/Svcstatus/granted</ServiceStatus></ServiceInformation>
            <ServiceHistory><ServiceHistoryInstance><ServiceStatus>http://x/withdrawn</ServiceStatus></ServiceHistoryInstance></ServiceHistory></TSPService>
            <TSPService><ServiceInformation><ServiceTypeIdentifier>http://uri.etsi.org/TrstSvc/Svctype/CA/QC</ServiceTypeIdentifier>
            <ServiceDigitalIdentity><DigitalId><X509Certificate>BAUG</X509Certificate></DigitalId></ServiceDigitalIdentity>
            <ServiceStatus>http://uri.etsi.org/TrstSvc/TrustedList/Svcstatus/withdrawn</ServiceStatus></ServiceInformation></TSPService>"#;
        let s = parse_services(tl, "Asseco");
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].kind, "ca");
        assert_eq!(s[0].certs, vec![vec![1u8, 2, 3]]);
    }

    /// Dopasowanie certyfikatów z magazynu użytkownika do listy PL (tylko odczyt; z siecią).
    #[test]
    fn local_certs_against_pl_list() {
        if std::env::var("PDFWB_TEST_LOCAL_CERTS").is_err() {
            return;
        }
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let pl = rt.block_on(tl_services(&reqwest::Client::new(), &std::env::temp_dir().join("pdfwb-tsl-test"), "PL")).unwrap();
        for c in certs_with_key().unwrap().iter().filter(|c| c.qualified()) {
            let m = tl_issuer(c, &pl, "ca");
            let rev = m.as_ref().map(|(_, ca)| revocation_with_issuer(c, ca));
            eprintln!("{} / {} -> {:?} rev={:?}", c.name(false), c.name(true), m.map(|x| x.0), rev);
        }
    }
}
