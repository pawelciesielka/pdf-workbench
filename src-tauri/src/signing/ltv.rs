//! PAdES-LT: dane do długoterminowej weryfikacji w słowniku /DSS dokumentu — certyfikaty
//! łańcuchów i listy unieważnień (CRL) dla podpisujących i serwerów znaczników czasu.
//! Dzięki nim podpis da się sprawdzić także po wygaśnięciu certyfikatów i zniknięciu CRL z sieci.

use super::*;

/// (długość nagłówka, długość zawartości) elementu DER.
fn der_header(d: &[u8]) -> Option<(usize, usize)> {
    if d.len() < 2 {
        return None;
    }
    let l = d[1] as usize;
    if l < 0x80 {
        return Some((2, l));
    }
    let n = l & 0x7f;
    if n == 0 || n > 4 || d.len() < 2 + n {
        return None;
    }
    Some((2 + n, d[2..2 + n].iter().fold(0usize, |a, b| (a << 8) | *b as usize)))
}

/// Token znacznika czasu zapisany w podpisie CMS jako atrybut niepodpisany.
pub(super) fn embedded_timestamp(cms: &[u8]) -> Option<&[u8]> {
    let p = cms.windows(DER_OID_TST.len()).position(|w| w == DER_OID_TST)? + DER_OID_TST.len();
    let set = cms.get(p..)?;
    if set.first() != Some(&0x31) {
        return None;
    }
    let (h, _) = der_header(set)?;
    let token = &set[h..];
    token.get(..der_len(token)?)
}

/// Wartość podpisu (zaszyfrowany skrót) z podpisu CMS — to ją obejmuje znacznik czasu podpisu.
pub(super) fn signature_value(cms: &[u8]) -> Option<Vec<u8>> {
    unsafe {
        let msg = Msg(CryptMsgOpenToDecode(ENC.0, CMSG_DETACHED_FLAG, 0, None, None, None));
        if msg.0.is_null() {
            return None;
        }
        CryptMsgUpdate(msg.0, Some(cms), true).ok()?;
        msg_param(msg.0, CMSG_ENCRYPTED_DIGEST).ok()
    }
}

/// Certyfikat podpisującego i magazyn z certyfikatami zawartymi w podpisie CMS (także w tokenie).
pub(super) fn cms_signer(cms: &[u8]) -> Option<(Store, Cert)> {
    unsafe {
        // Token RFC 3161 ma treść w środku, zwykły podpis PAdES — odłączoną; certyfikaty odczytamy w obu trybach.
        let msg = Msg(CryptMsgOpenToDecode(ENC.0, 0, 0, None, None, None));
        if msg.0.is_null() {
            return None;
        }
        CryptMsgUpdate(msg.0, Some(cms), true).ok()?;
        let info = msg_param_struct(msg.0, CMSG_SIGNER_CERT_INFO_PARAM).ok()?;
        let store = Store(CertOpenStore(CERT_STORE_PROV_MSG, ENC, None, CERT_OPEN_STORE_FLAGS(0), Some(msg.0 as *const c_void)).ok()?);
        let c = CertGetSubjectCertificateFromStore(store.0, ENC, info.as_ptr() as *const CERT_INFO);
        if c.is_null() {
            return None;
        }
        Some((store, Cert(c)))
    }
}

pub(super) fn self_signed(c: &Cert) -> bool {
    let i = c.info();
    unsafe { blob(&i.Subject) == blob(&i.Issuer) }
}

/// Łańcuch certyfikatów (od podpisującego do głównego) i flagi błędów zaufania Windows.
pub(super) fn chain_of(cert: &Cert, extra: Option<HCERTSTORE>, time: Option<FILETIME>, flags: u32) -> (Vec<Cert>, u32) {
    let mut out = Vec::new();
    unsafe {
        let mut para: CERT_CHAIN_PARA = std::mem::zeroed();
        para.cbSize = std::mem::size_of::<CERT_CHAIN_PARA>() as u32;
        let mut ctx: *mut CERT_CHAIN_CONTEXT = null_mut();
        let t = time.as_ref().map(|t| t as *const FILETIME);
        if CertGetCertificateChain(None, cert.0, t, extra, &para, flags, None, &mut ctx).is_err() || ctx.is_null() {
            return (vec![Cert(CertDuplicateCertificateContext(Some(cert.0)))], u32::MAX);
        }
        let status = (*ctx).TrustStatus.dwErrorStatus;
        if (*ctx).cChain > 0 {
            let simple = *(*ctx).rgpChain;
            for i in 0..(*simple).cElement as usize {
                let el = *(*simple).rgpElement.add(i);
                out.push(Cert(CertDuplicateCertificateContext(Some((*el).pCertContext))));
            }
        }
        CertFreeCertificateChain(ctx);
        (out, status)
    }
}

/// Adresy HTTP (lub file:) list unieważnień z rozszerzenia CRL Distribution Points.
fn crl_urls(cert: &Cert) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let i = cert.info();
        if i.rgExtension.is_null() || i.cExtension == 0 {
            return out;
        }
        let exts = std::slice::from_raw_parts(i.rgExtension, i.cExtension as usize);
        let e = CertFindExtension(PCSTR(b"2.5.29.31\0".as_ptr()), exts);
        if e.is_null() {
            return out;
        }
        let v = blob(&(*e).Value);
        // GeneralName uniformResourceIdentifier: [6] IMPLICIT IA5String.
        let mut k = 0;
        while k + 2 < v.len() {
            if v[k] == 0x86 && (v[k + 1] as usize) < 0x80 {
                let n = v[k + 1] as usize;
                if let Some(s) = v.get(k + 2..k + 2 + n).and_then(|b| std::str::from_utf8(b).ok()) {
                    if s.starts_with("http") || s.starts_with("file:") {
                        out.push(s.to_string());
                    }
                }
                k += 2 + n;
            } else {
                k += 1;
            }
        }
    }
    out
}

/// Pobiera CRL przez Windows (z pamięcią podręczną systemu).
fn fetch_crl(url: &str) -> Option<Vec<u8>> {
    let w: Vec<u16> = url.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let mut obj: *mut c_void = null_mut();
        if !CryptRetrieveObjectByUrlW(PCWSTR(w.as_ptr()), CONTEXT_OID_CRL, 0, 30_000, &mut obj, None, None, None, None).as_bool() || obj.is_null() {
            return None;
        }
        let crl = obj as *const CRL_CONTEXT;
        let bytes = std::slice::from_raw_parts((*crl).pbCrlEncoded, (*crl).cbCrlEncoded as usize).to_vec();
        let _ = CertFreeCRLContext(Some(crl));
        Some(bytes)
    }
}

/// Certyfikaty i CRL potrzebne do sprawdzenia wszystkich podpisów i znaczników czasu w pliku.
pub(super) fn validation_material(bytes: &[u8]) -> Res<(Vec<Vec<u8>>, Vec<Vec<u8>>)> {
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    for r in raw_signatures(bytes) {
        if let Some(t) = embedded_timestamp(&r.cms) {
            tokens.push(t.to_vec());
        }
        tokens.push(r.cms);
    }
    let (mut certs, mut crls, mut missing) = (Vec::<Vec<u8>>::new(), Vec::<Vec<u8>>::new(), Vec::<String>::new());
    for cms in &tokens {
        let Some((store, signer)) = cms_signer(cms) else { continue };
        let (chain, _) = chain_of(&signer, Some(store.0), None, 0);
        for c in &chain {
            let der = c.der().to_vec();
            if certs.contains(&der) {
                continue;
            }
            certs.push(der);
            if self_signed(c) {
                continue; // certyfikat główny nie ma listy unieważnień
            }
            match crl_urls(c).iter().find_map(|u| fetch_crl(u)) {
                Some(crl) if !crls.contains(&crl) => crls.push(crl),
                Some(_) => {}
                None => missing.push(c.name(false)),
            }
        }
    }
    if certs.is_empty() {
        return Err("w pliku nie ma podpisów do zabezpieczenia".into());
    }
    if !missing.is_empty() {
        return Err(format!("nie udało się pobrać listy unieważnień (CRL) dla: {}", missing.join(", ")));
    }
    Ok((certs, crls))
}

/// Dopisuje (przyrostowo) słownik /DSS z certyfikatami i CRL wszystkich podpisów w pliku.
pub(super) fn add_ltv(bytes: &[u8]) -> Res<Vec<u8>> {
    let (certs, crls) = validation_material(bytes)?;
    let mut up = Update::open(bytes)?;
    let mut dss = resolve_dict(&up.doc, up.catalog.get(b"DSS").ok());
    let mut cert_refs = resolve_array(&up.doc, dss.get(b"Certs").ok());
    let mut crl_refs = resolve_array(&up.doc, dss.get(b"CRLs").ok());
    for c in &certs {
        cert_refs.push(Object::Reference(up.put_stream(c)));
    }
    for c in &crls {
        crl_refs.push(Object::Reference(up.put_stream(c)));
    }
    dss.set("Type", name("DSS"));
    dss.set("Certs", Object::Array(cert_refs));
    if !crl_refs.is_empty() {
        dss.set("CRLs", Object::Array(crl_refs));
    }
    up.catalog.set("DSS", Object::Dictionary(dss));
    up.catalog_dirty = true;
    up.finish()
}

/// Czy plik ma słownik /DSS z danymi o unieważnieniach (CRL albo OCSP).
pub(super) fn has_ltv(bytes: &[u8]) -> bool {
    find(bytes, b"/DSS").is_some() && (find(bytes, b"/CRLs").is_some() || find(bytes, b"/OCSPs").is_some())
}
