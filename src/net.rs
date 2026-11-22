//! GET por HTTP(S) con WinHTTP (incluido en Windows, sin dependencias), para los widgets de noticias y
//! bolsa. Como `http_get` de main.rs, pero con agente genérico, plazos cortos, código de estado y tope de
//! tamaño. Solo se manda la URL: ni cookies ni nada del usuario.

/// Agente genérico (sin nombre de la app ni versión: no identifica a nadie).
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";
/// Tope de lo que se lee (un feed o un gráfico del día ocupan mucho menos).
const MAX: usize = 4 << 20;

/// Partes de una URL: (seguro, servidor, puerto, ruta con la consulta).
pub(crate) fn split_url(url: &str) -> Option<(bool, String, u16, String)> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") { (true, r) } else { (false, url.strip_prefix("http://")?) };
    let cut = rest.find(['/', '?']).unwrap_or(rest.len());
    let (hostport, path) = rest.split_at(cut);
    let path = if path.is_empty() { "/".to_string() } else if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    let def = if secure { 443 } else { 80 };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !h.is_empty() => (h, p.parse().ok()?),
        _ => (hostport, def),
    };
    if host.is_empty() || host.contains(|c: char| c.is_whitespace() || c == '@') { return None; }
    Some((secure, host.to_string(), port, path))
}

/// Por qué falló una petición.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fail {
    /// La dirección no se entiende (o no es http/https).
    Url,
    /// Sin respuesta: sin red, el servidor no existe o tardó demasiado.
    Net,
    /// El servidor contestó con ese código (404, 500…).
    Status(u32),
    /// Más grande que el tope.
    TooBig,
}

impl Fail {
    /// El error en una frase, para enseñarlo.
    pub(crate) fn message(self) -> String {
        match self {
            Fail::Url => crate::i18n::t("La dirección no es válida").into(),
            Fail::Net => crate::i18n::t("No responde (¿sin conexión?)").into(),
            Fail::Status(404) => crate::i18n::t("No existe (error 404)").into(),
            Fail::Status(c) => crate::i18n::tf("El servidor contestó con un error ({})", &[&c]),
            Fail::TooBig => crate::i18n::t("Es demasiado grande").into(),
        }
    }
}

/// GET: el cuerpo si el servidor contesta 2xx.
pub(crate) fn get(url: &str) -> Option<Vec<u8>> { fetch(url).ok() }

/// GET con el motivo si falla.
pub(crate) fn fetch(url: &str) -> Result<Vec<u8>, Fail> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Networking::WinHttp::*;
    let (secure, host, port, path) = split_url(url).ok_or(Fail::Url)?;
    unsafe {
        let s = WinHttpOpen(&HSTRING::from(UA), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if s.is_null() { return Err(Fail::Net); }
        // Resolver, conectar, enviar y recibir: nada de colgarse un minuto sin red
        let _ = WinHttpSetTimeouts(s, 5000, 6000, 6000, 10000);
        let c = WinHttpConnect(s, &HSTRING::from(host.as_str()), port, 0);
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let r = if c.is_null() { std::ptr::null_mut() } else {
            WinHttpOpenRequest(c, &HSTRING::from("GET"), &HSTRING::from(path.as_str()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags)
        };
        let mut out = Vec::new();
        let mut res: Result<(), Fail> = if !r.is_null()
            && WinHttpSendRequest(r, None, None, 0, 0, 0).is_ok()
            && WinHttpReceiveResponse(r, std::ptr::null_mut()).is_ok() { Ok(()) } else { Err(Fail::Net) };
        if res.is_ok() {
            let mut code = 0u32;
            let mut len = 4u32;
            let q = WinHttpQueryHeaders(r, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut code as *mut u32 as _), &mut len, std::ptr::null_mut());
            if q.is_err() { res = Err(Fail::Net); } else if !(200..300).contains(&code) { res = Err(Fail::Status(code)); }
        }
        if res.is_ok() {
            loop {
                let mut avail = 0u32;
                if WinHttpQueryDataAvailable(r, &mut avail).is_err() || avail == 0 { break; }
                let mut buf = vec![0u8; avail as usize];
                let mut read = 0u32;
                if WinHttpReadData(r, buf.as_mut_ptr() as _, avail, &mut read).is_err() { res = Err(Fail::Net); break; }
                out.extend_from_slice(&buf[..read as usize]);
                if out.len() > MAX { res = Err(Fail::TooBig); break; }
            }
        }
        for h in [r, c, s] { if !h.is_null() { let _ = WinHttpCloseHandle(h); } }
        res.map(|_| out)
    }
}

/// Codifica para ir dentro de una ruta o consulta (todo lo que no sea letra, cifra o `-._~`).
pub(crate) fn encode(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { o.push(b as char); } else { o += &format!("%{b:02X}"); }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partes_de_la_url() {
        assert_eq!(split_url("https://news.google.com/rss?hl=es&gl=ES"), Some((true, "news.google.com".into(), 443, "/rss?hl=es&gl=ES".into())));
        assert_eq!(split_url("http://ejemplo.org:8080/feed.xml"), Some((false, "ejemplo.org".into(), 8080, "/feed.xml".into())));
        assert_eq!(split_url("https://ejemplo.org"), Some((true, "ejemplo.org".into(), 443, "/".into())));
        assert_eq!(split_url("https://ejemplo.org?x=1"), Some((true, "ejemplo.org".into(), 443, "/?x=1".into())));
        assert_eq!(split_url("ftp://ejemplo.org/a"), None);
        assert_eq!(split_url("https:///a"), None);
        assert_eq!(split_url("https://user@host/a"), None);
    }

    #[test]
    fn codificar() {
        assert_eq!(encode("^IBEX"), "%5EIBEX");
        assert_eq!(encode("SAN.MC"), "SAN.MC");
        assert_eq!(encode("BTC-USD"), "BTC-USD");
        assert_eq!(encode("EURUSD=X"), "EURUSD%3DX");
    }

    /// Prueba de red de verdad (manual): `cargo test --release --offline -- --ignored red_de_verdad --nocapture`
    #[test]
    #[ignore]
    fn red_de_verdad() {
        let rss = get(crate::news::DEFAULT_FEED).expect("Google News responde");
        let items = crate::news::parse_feed(&crate::news::decode_text(&rss));
        println!("Google News: {} titulares; primero: {:?}", items.len(), items.first());
        assert!(items.len() >= 5);
        for sym in ["^IBEX", "SAN.MC", "AAPL", "BTC-USD"] {
            let body = get(&crate::stocks::chart_url(sym)).expect("Yahoo responde");
            let q = crate::stocks::parse_chart(&String::from_utf8_lossy(&body), sym).expect("se entiende");
            println!("{sym}: {} {} ({:+.2} %) {} puntos", q.price, q.currency, q.change_pct(), q.closes.len());
            assert!(q.price > 0.0 && !q.closes.is_empty());
        }
    }
}
