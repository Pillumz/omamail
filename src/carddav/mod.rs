//! Generic read-only CardDAV address books (RFC 6352).
//!
//! Rust owns every network, credential, XML and vCard decision here; the UI
//! receives only bounded contact snapshots. A source is an explicit HTTPS URL,
//! a username and a keyring password typed by the user. Credentials are looked
//! up by the derived source id only after the destination URL validates, are
//! sent with HTTP Basic to that origin alone, and redirects are followed by
//! hand only while they stay on the same origin. Nothing writes to the server:
//! the only methods spoken are PROPFIND and the addressbook-query REPORT.

use quick_xml::{
    Reader,
    escape::resolve_xml_entity,
    events::{BytesRef, Event},
};
use reqwest::{Method, Response, StatusCode, Url, header::LOCATION};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{sync::OnceLock, time::Duration};

const DISCOVERY_LIMIT: usize = 4 * 1024 * 1024;
const BOOK_LIMIT: usize = 16 * 1024 * 1024;
const SNAPSHOT_LIMIT: usize = 16 * 1024 * 1024;
const SNAPSHOT_READ_LIMIT: u64 = 17 * 1024 * 1024;
const MAX_BOOKS: usize = 64;
const MAX_HOMES: usize = 8;
const MAX_REDIRECTS: usize = 4;
const MAX_CARDS: usize = 10_000;
const MAX_CARD_BYTES: usize = 1024 * 1024;
const MAX_CARD_EMAILS: usize = 100;
const MAX_CONTACTS: usize = 10_000;
const MAX_NAME_BYTES: usize = 1024;
const MAX_EMAIL_BYTES: usize = 254;
const MAX_XML_DEPTH: usize = 64;
const MAX_SNAPSHOT_CONTACTS: usize = 50_000;

static CLIENT: OnceLock<Result<reqwest::Client, &'static str>> = OnceLock::new();

fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .https_only(true)
        .hickory_dns(true)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
}

fn client() -> Result<&'static reqwest::Client, &'static str> {
    CLIENT
        .get_or_init(|| client_builder().build().map_err(|_| "carddav_network_failed"))
        .as_ref()
        .map_err(|e| *e)
}

/// The canonical form of a source URL: https, printable ASCII only,
/// scheme and host lowercased, the default port elided, an empty path
/// shown as "/". Everything else is kept verbatim, so two spellings share
/// an identity only when they name the same endpoint. Computed by hand
/// rather than through `url::Url` so the QML settings code derives the
/// exact same string (`CarddavSources.canonicalParts` mirrors this).
fn canonical_parts(raw: &str) -> Option<(String, String)> {
    let text = raw.trim();
    if text.is_empty()
        || text.len() > 2048
        || !text.is_ascii()
        || text.contains('\\')
        || text.bytes().any(|b| !(0x21..=0x7e).contains(&b))
    {
        return None;
    }
    if text.len() < 9 || !text[..8].eq_ignore_ascii_case("https://") {
        return None;
    }
    let rest = &text[8..];
    let path_start = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..path_start];
    let tail = &rest[path_start..];
    if authority.is_empty() || authority.contains('@') || tail.contains(['?', '#']) {
        return None;
    }
    let (host, port) = if authority.starts_with('[') {
        let close = authority.find(']')?;
        let host = &authority[..=close];
        let after = &authority[close + 1..];
        if after.is_empty() {
            (host, None)
        } else if let Some(port) = after.strip_prefix(':') {
            (host, Some(port))
        } else {
            return None;
        }
    } else {
        match authority.rfind(':') {
            Some(index) => (&authority[..index], Some(&authority[index + 1..])),
            None => (authority, None),
        }
    };
    let host = host.to_lowercase();
    if host.is_empty() || host.contains(':') {
        return None;
    }
    let port = match port {
        Some(port) => {
            if port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let value: u32 = port.parse().ok()?;
            if value > 65535 {
                return None;
            }
            if value == 443 { String::new() } else { format!(":{value}") }
        }
        None => String::new(),
    };
    let path = if tail.is_empty() { "/" } else { tail };
    let origin = format!("https://{host}{port}");
    Some((origin.clone(), format!("{origin}{path}")))
}

fn source_url(raw: &str) -> Result<Url, &'static str> {
    if canonical_parts(raw).is_none() {
        return Err("carddav_invalid_url");
    }
    let url = Url::parse(raw.trim()).map_err(|_| "carddav_invalid_url")?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
    {
        return Err("carddav_invalid_url");
    }
    Ok(url)
}

/// The stable identity of one address book source: the SHA-256 of the
/// canonical endpoint and the username. Two spellings of one endpoint
/// (`https://dav.example.org`, `https://dav.example.org:443/`) share an id
/// and therefore one keyring entry; two endpoints whose old slug spelling
/// could collide (`https://a.b` against `https://a-b`, or two accounts on
/// one server) never do. `CarddavSources.deriveId` mirrors this exactly.
pub(crate) fn derive_source_id(raw: &str, username: &str) -> Option<String> {
    let (_, canonical) = canonical_parts(raw)?;
    let key = format!("{canonical}\n{}", username.trim());
    let mut id = String::from("carddav:");
    for byte in Sha256::digest(key.as_bytes()) {
        id.push_str(&format!("{byte:02x}"));
    }
    Some(id)
}

fn bounded(value: &str, limit: usize) -> Result<&str, &'static str> {
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        return Err("invalid_params");
    }
    Ok(value)
}

struct Source {
    id: String,
    url: Url,
    username: String,
}

fn valid_source_id(id: &str) -> Result<&str, &'static str> {
    if id.len() == 72
        && id.starts_with("carddav:")
        && id[8..]
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Ok(id)
    } else {
        Err("invalid_params")
    }
}

fn source(params: &Value) -> Result<Source, &'static str> {
    let value = params["source"].as_object().ok_or("invalid_params")?;
    if value.len() != 3
        || value
            .keys()
            .any(|key| !["id", "url", "username"].contains(&key.as_str()))
    {
        return Err("invalid_params");
    }
    let id = valid_source_id(bounded(value["id"].as_str().ok_or("invalid_params")?, 1024)?)?;
    let raw_url = bounded(value["url"].as_str().ok_or("invalid_params")?, 2048)?;
    let username = bounded(value["username"].as_str().ok_or("invalid_params")?, 1024)?;
    if username.chars().any(char::is_whitespace) {
        return Err("invalid_params");
    }
    // A source record whose id does not derive from its URL and username
    // could pair an arbitrary endpoint with a password stored for another.
    // The binding is checked before any credential or snapshot lookup.
    if derive_source_id(raw_url, username).as_deref() != Some(id) {
        return Err("invalid_params");
    }
    Ok(Source {
        id: id.to_owned(),
        url: source_url(raw_url)?,
        username: username.to_owned(),
    })
}

/// A server-supplied href. Every href is resolved against the URL it was
/// asked from and must stay on that origin before it is requested or stored.
fn resolve(base: &Url, href: &str) -> Result<Url, &'static str> {
    if href.is_empty()
        || href.len() > 8192
        || href.chars().any(char::is_control)
        || href.contains('\\')
    {
        return Err("carddav_invalid_response");
    }
    let url = base.join(href).map_err(|_| "carddav_invalid_response")?;
    if url.origin() != base.origin()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("carddav_origin_refused");
    }
    Ok(url)
}

async fn password_with<F, Fut>(
    source_id: &str,
    lookup: F,
) -> Result<crate::credentials::Secret, &'static str>
where
    F: FnOnce(crate::credentials::CredentialKey) -> Fut,
    Fut: std::future::Future<
        Output = Result<crate::credentials::Secret, crate::credentials::Error>,
    >,
{
    let key = crate::credentials::CredentialKey {
        provider: "carddav".into(),
        account_id: source_id.to_owned(),
        kind: crate::credentials::CredentialKind::CarddavPassword,
    };
    let secret = lookup(key).await.map_err(|error| match error {
        crate::credentials::Error::Missing => "carddav_password_missing",
        crate::credentials::Error::InvalidKey | crate::credentials::Error::InvalidSecret => {
            "carddav_password_invalid"
        }
        _ => "carddav_keyring_failed",
    })?;
    let text = secret.text().map_err(|_| "carddav_password_invalid")?;
    if text.is_empty() || text.chars().any(char::is_control) {
        return Err("carddav_password_invalid");
    }
    Ok(secret)
}

async fn response_body(response: Response, limit: usize) -> Result<String, &'static str> {
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err("carddav_response_too_large");
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "carddav_network_failed")?
    {
        if chunk.len() > limit - bytes.len() {
            return Err("carddav_response_too_large");
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|_| "carddav_invalid_response")
}

/// One PROPFIND or REPORT, with redirects followed by hand: never more than
/// four, never off the origin the password belongs to. reqwest's own redirect
/// policy is `none`, so a 3xx we do not accept cannot be followed at all.
async fn dav_with(
    client: &reqwest::Client,
    mut url: Url,
    method: &Method,
    depth: &str,
    body: &'static str,
    username: &str,
    password: &str,
    limit: usize,
) -> Result<(Url, String), &'static str> {
    for _ in 0..MAX_REDIRECTS {
        let response = client
            .request(method.clone(), url.clone())
            .basic_auth(username, Some(password))
            .header("Depth", depth)
            .header("Content-Type", "application/xml; charset=utf-8")
            .body(body)
            .send()
            .await
            .map_err(|_| "carddav_network_failed")?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or("carddav_invalid_response")?;
            url = resolve(&url, location)?;
            continue;
        }
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err("carddav_auth_refused");
        }
        if response.status() == StatusCode::NOT_FOUND {
            return Err("carddav_not_found");
        }
        if response.status() != StatusCode::MULTI_STATUS && !response.status().is_success() {
            return Err("carddav_request_failed");
        }
        return Ok((url, response_body(response, limit).await?));
    }
    Err("carddav_too_many_redirects")
}

// ---------------------------------------------------------------------------
// Bounded XML reading. quick_xml never resolves document-defined entities;
// character references and the five predefined entities are resolved by hand,
// a DOCTYPE is refused outright, and the open-element stack is depth-capped.

fn local_name(raw: &[u8]) -> &[u8] {
    raw.rsplit(|byte| *byte == b':').next().unwrap_or(raw)
}

fn append_reference(target: &mut String, reference: &BytesRef<'_>) -> Result<(), &'static str> {
    if let Some(value) = reference
        .resolve_char_ref()
        .map_err(|_| "carddav_invalid_response")?
    {
        target.push(value);
        return Ok(());
    }
    let name = reference
        .decode()
        .map_err(|_| "carddav_invalid_response")?;
    target.push_str(resolve_xml_entity(&name).ok_or("carddav_invalid_response")?);
    Ok(())
}

/// The hrefs nested inside one named property, e.g. current-user-principal or
/// addressbook-home-set. A multistatus also carries a response-level href, so
/// the container scope matters.
fn nested_hrefs(xml: &str, container: &[u8]) -> Result<Vec<String>, &'static str> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<Vec<u8>> = Vec::new();
    let mut text = String::new();
    let mut hrefs = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                if stack.len() >= MAX_XML_DEPTH {
                    return Err("carddav_invalid_response");
                }
                stack.push(local_name(event.name().as_ref()).to_vec());
                if stack.last().is_some_and(|name| name.as_slice() == b"href") {
                    text.clear();
                }
            }
            Ok(Event::Text(event))
                if stack.last().is_some_and(|name| name.as_slice() == b"href") =>
            {
                text.push_str(&event.decode().map_err(|_| "carddav_invalid_response")?);
            }
            Ok(Event::CData(event))
                if stack.last().is_some_and(|name| name.as_slice() == b"href") =>
            {
                text.push_str(&event.decode().map_err(|_| "carddav_invalid_response")?);
            }
            Ok(Event::GeneralRef(event))
                if stack.last().is_some_and(|name| name.as_slice() == b"href") =>
            {
                append_reference(&mut text, &event)?;
            }
            Ok(Event::End(event)) => {
                let qualified = event.name();
                let name = local_name(qualified.as_ref()).to_vec();
                if name == b"href"
                    && stack.iter().any(|entry| entry.as_slice() == container)
                    && !text.trim().is_empty()
                {
                    if hrefs.len() >= MAX_HOMES {
                        return Err("carddav_invalid_response");
                    }
                    hrefs.push(text.trim().to_owned());
                }
                stack.pop();
            }
            Ok(Event::DocType(_)) => return Err("carddav_invalid_response"),
            Ok(Event::Eof) => return Ok(hrefs),
            Err(_) => return Err("carddav_invalid_response"),
            _ => {}
        }
    }
}

#[derive(Default)]
struct DavBook {
    href: String,
    name: String,
    is_addressbook: bool,
}

/// The address books in a Depth:1 PROPFIND answer, as absolute same-origin
/// URLs. Responses whose resourcetype lacks `addressbook` — the home set
/// itself, calendar collections, principals — are skipped.
fn book_list(xml: &str, base: &Url) -> Result<Vec<Value>, &'static str> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut current: Option<DavBook> = None;
    let mut capture = Vec::<u8>::new();
    let mut captured = String::new();
    let mut books = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref()).to_vec();
                if name == b"response" {
                    current = Some(DavBook::default());
                }
                if let Some(value) = current.as_mut() {
                    match name.as_slice() {
                        b"addressbook" => value.is_addressbook = true,
                        b"href" | b"displayname" => {
                            capture = name;
                            captured.clear();
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Empty(event)) => {
                if let Some(value) = current.as_mut()
                    && local_name(event.name().as_ref()) == b"addressbook"
                {
                    value.is_addressbook = true;
                }
            }
            Ok(Event::Text(event)) if !capture.is_empty() => {
                captured.push_str(&event.decode().map_err(|_| "carddav_invalid_response")?);
            }
            Ok(Event::CData(event)) if !capture.is_empty() => {
                captured.push_str(&event.decode().map_err(|_| "carddav_invalid_response")?);
            }
            Ok(Event::GeneralRef(event)) if !capture.is_empty() => {
                append_reference(&mut captured, &event)?;
            }
            Ok(Event::End(event)) => {
                let qualified = event.name();
                let name = local_name(qualified.as_ref()).to_vec();
                if name == capture.as_slice() {
                    // Propstats answer independently: a 404 propstat repeats
                    // the absent property as an empty element, and it may
                    // follow the 200 one. The first non-empty value wins.
                    if let Some(value) = current.as_mut() {
                        if name == b"href" && value.href.is_empty() {
                            value.href = captured.trim().to_owned();
                        } else if name == b"displayname"
                            && value.name.is_empty()
                            && !captured.trim().is_empty()
                        {
                            value.name = captured.trim().to_owned();
                        }
                    }
                    capture.clear();
                    captured.clear();
                }
                if name == b"response" {
                    let value = current.take().ok_or("carddav_invalid_response")?;
                    if value.is_addressbook {
                        let url = resolve(base, &value.href)?;
                        if books.len() >= MAX_BOOKS {
                            return Err("carddav_too_many_books");
                        }
                        let name = if value.name.is_empty()
                            || value.name.len() > 1024
                            || value.name.chars().any(char::is_control)
                        {
                            "Address book".to_owned()
                        } else {
                            value.name
                        };
                        books.push(json!({"href": url.as_str(), "name": name}));
                    }
                }
            }
            Ok(Event::DocType(_)) => return Err("carddav_invalid_response"),
            Ok(Event::Eof) => return Ok(books),
            Err(_) => return Err("carddav_invalid_response"),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// vCard reading. Cards arrive as address-data text inside the multistatus.
// Folding is undone first (CRLF + one space or tab), parameters are read for
// the v3 ENCODING/CHARSET pair, quoted parameter values can hold a colon, and
// FN/EMAIL values are unescaped. PHOTO and every other property are ignored;
// no resource a card names is ever fetched.

/// Content lines: physical lines joined where a continuation starts with
/// one space or tab. A quoted-printable value additionally marks its own
/// soft breaks with a trailing '=' (a literal '=' is encoded as =3D, so a
/// trailing '=' before a fold is always a soft break), and the soft break
/// disappears with the fold. Folding anywhere else, including between the
/// hex pairs of one encoded character, is a plain join.
fn logical_lines(card: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut quoted_printable: Vec<bool> = Vec::new();
    for line in card.replace("\r\n", "\n").replace('\r', "\n").split('\n') {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            if let Some(last) = lines.last_mut() {
                if quoted_printable.last() == Some(&true) && last.ends_with('=') {
                    last.pop();
                }
                last.push_str(&line[1..]);
            }
            continue;
        }
        let head = match split_property(line) {
            Some((head, _)) => head,
            None => line,
        };
        quoted_printable.push(
            head.to_ascii_uppercase()
                .contains("ENCODING=QUOTED-PRINTABLE"),
        );
        lines.push(line.to_string());
    }
    lines
}

fn split_property(line: &str) -> Option<(&str, &str)> {
    let mut quoted = false;
    for (index, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some((&line[..index], &line[index + 1..])),
            _ => {}
        }
    }
    None
}

fn decode_quoted_printable(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 3 <= bytes.len() {
            if let Some(Ok(byte)) = std::str::from_utf8(&bytes[i + 1..i + 3])
                .ok()
                .map(|hex| u8::from_str_radix(hex, 16))
            {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    out
}

fn decode_value(head: &str, value: &str) -> String {
    let mut quoted_printable = false;
    let mut charset = None;
    for param in head.split(';').skip(1) {
        let Some((name, raw)) = param.split_once('=') else {
            continue;
        };
        let raw = raw.trim_matches('"');
        if name.eq_ignore_ascii_case("encoding") && raw.eq_ignore_ascii_case("quoted-printable") {
            quoted_printable = true;
        } else if name.eq_ignore_ascii_case("charset") {
            charset = Some(raw.to_owned());
        }
    }
    let text = if quoted_printable {
        let bytes = decode_quoted_printable(value);
        match charset.as_deref().and_then(|label| encoding_rs::Encoding::for_label(label.as_bytes())) {
            Some(encoding) => encoding.decode(&bytes).0.into_owned(),
            None => String::from_utf8_lossy(&bytes).into_owned(),
        }
    } else {
        value.to_owned()
    };
    unescape(&text)
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') | Some('N') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

/// One card becomes one row per email address, like the local vCard source.
/// Names and addresses are bounded here exactly as the snapshot reader
/// bounds them, so a book that was written can always be read back.
fn card_contacts(
    book: &mut Vec<Value>,
    card: &str,
    seen: &mut std::collections::HashSet<String>,
) -> Result<(), &'static str> {
    if card.len() > MAX_CARD_BYTES {
        return Err("carddav_too_many_contacts");
    }
    let mut name = String::new();
    let mut emails = Vec::new();
    for line in logical_lines(card) {
        let line = line.trim();
        let Some((head, value)) = split_property(line) else {
            continue;
        };
        let property = head.split(';').next().unwrap_or("");
        if property.eq_ignore_ascii_case("FN") {
            if name.is_empty() {
                name = decode_value(head, value.trim());
            }
        } else if property.eq_ignore_ascii_case("EMAIL") {
            if emails.len() >= MAX_CARD_EMAILS {
                return Err("carddav_too_many_contacts");
            }
            emails.push(decode_value(head, value.trim()));
        }
    }
    let name: String = name.trim().chars().filter(|c| !c.is_control()).collect();
    let mut boundary = name.len().min(MAX_NAME_BYTES);
    while !name.is_char_boundary(boundary) {
        boundary -= 1;
    }
    let name = &name[..boundary];
    for email in emails {
        let email = email.trim();
        if email.len() > MAX_EMAIL_BYTES || !crate::contacts::address(email) {
            continue;
        }
        if seen.len() >= MAX_CONTACTS {
            return Err("carddav_too_many_contacts");
        }
        if seen.insert(email.to_lowercase()) {
            book.push(json!({"name": name, "email": email}));
        }
    }
    Ok(())
}

/// Every vCard in an addressbook-query answer. Hitting a bound is an error,
/// never a silently truncated book.
fn book_contacts(xml: &str) -> Result<Vec<Value>, &'static str> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut depth = 0usize;
    let mut in_data = false;
    let mut data = String::new();
    let mut book = Vec::new();
    let mut cards = 0usize;
    let mut seen = std::collections::HashSet::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                depth += 1;
                if depth > MAX_XML_DEPTH {
                    return Err("carddav_invalid_response");
                }
                if local_name(event.name().as_ref()) == b"address-data" {
                    in_data = true;
                    data.clear();
                }
            }
            Ok(Event::Text(event)) if in_data => {
                if event.len() > MAX_CARD_BYTES - data.len() {
                    return Err("carddav_too_many_contacts");
                }
                data.push_str(&event.decode().map_err(|_| "carddav_invalid_response")?);
            }
            Ok(Event::CData(event)) if in_data => {
                if event.len() > MAX_CARD_BYTES - data.len() {
                    return Err("carddav_too_many_contacts");
                }
                data.push_str(&event.decode().map_err(|_| "carddav_invalid_response")?);
            }
            Ok(Event::GeneralRef(event)) if in_data => {
                append_reference(&mut data, &event)?;
            }
            Ok(Event::End(event)) => {
                if local_name(event.name().as_ref()) == b"address-data" && in_data {
                    in_data = false;
                    // A propstat for absent properties can spell address-data
                    // as an empty element; it is not a card.
                    if !data.trim().is_empty() {
                        cards += 1;
                        if cards > MAX_CARDS {
                            return Err("carddav_too_many_contacts");
                        }
                        card_contacts(&mut book, &data, &mut seen)?;
                        data.clear();
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Ok(Event::DocType(_)) => return Err("carddav_invalid_response"),
            Ok(Event::Eof) => return Ok(book),
            Err(_) => return Err("carddav_invalid_response"),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Snapshots: the last complete answer per source, kept in the private cache
// so a failed or impossible refresh leaves suggestions intact. One file per
// source id, written atomically; reads are bounded and re-validated.

fn snapshot_name(source_id: &str) -> String {
    format!("source-{:x}.json", Sha256::digest(source_id.as_bytes()))
}

fn snapshot_dir(create: bool) -> Result<Option<std::fs::File>, &'static str> {
    #[cfg(test)]
    if let Some(root) = SNAPSHOT_ROOT.lock().unwrap().clone() {
        return crate::platform::private_fs::directories(&root, &["carddav"], create)
            .map_err(|_| "carddav_snapshot_unavailable");
    }
    let dirs = crate::platform::dirs::AppDirs::discover()?;
    crate::platform::private_fs::directories(&dirs.cache, &["omamail", "carddav"], create)
        .map_err(|_| "carddav_snapshot_unavailable")
}

// Snapshot tests serialize on this lock: the override root is process-wide.
#[cfg(test)]
static SNAPSHOT_ROOT: std::sync::Mutex<Option<std::path::PathBuf>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
fn snapshot_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap()
}

#[cfg(test)]
fn snapshot_test_root() -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "omamail-carddav-snapshot-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    *SNAPSHOT_ROOT.lock().unwrap() = Some(root.clone());
    root
}

fn snapshot_read(source_id: &str) -> Result<Value, &'static str> {
    let Some(dir) = snapshot_dir(false)? else {
        return Ok(json!({"books": []}));
    };
    let name = snapshot_name(source_id);
    let Some(file) = crate::platform::private_fs::regular_readonly(&dir, &name)
        .map_err(|_| "carddav_snapshot_unavailable")?
    else {
        return Ok(json!({"books": []}));
    };
    if file
        .metadata()
        .map_err(|_| "carddav_snapshot_unavailable")?
        .len()
        > SNAPSHOT_READ_LIMIT
    {
        return Err("carddav_snapshot_unavailable");
    }
    let mut bytes = Vec::new();
    use std::io::Read;
    file.take(SNAPSHOT_READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "carddav_snapshot_unavailable")?;
    if bytes.len() as u64 > SNAPSHOT_READ_LIMIT {
        return Err("carddav_snapshot_unavailable");
    }
    let raw: Value =
        serde_json::from_slice(&bytes).map_err(|_| "carddav_snapshot_unavailable")?;
    Ok(json!({"books": snapshot_books(&raw)?}))
}

/// A snapshot is a cache of data the server already proved; still, it is a
/// file read back later, so every value is re-validated and re-bounded.
fn snapshot_books(raw: &Value) -> Result<Vec<Value>, &'static str> {
    if raw["version"] != 1 {
        return Err("carddav_snapshot_unavailable");
    }
    let mut books = Vec::new();
    let mut total = 0usize;
    for book in raw["books"].as_array().ok_or("carddav_snapshot_unavailable")? {
        if books.len() >= MAX_BOOKS {
            return Err("carddav_snapshot_unavailable");
        }
        let href = book["href"].as_str().ok_or("carddav_snapshot_unavailable")?;
        bounded(href, 8192).map_err(|_| "carddav_snapshot_unavailable")?;
        let mut contacts = Vec::new();
        for contact in book["contacts"]
            .as_array()
            .ok_or("carddav_snapshot_unavailable")?
        {
            if contacts.len() >= MAX_CONTACTS || total >= MAX_SNAPSHOT_CONTACTS {
                return Err("carddav_snapshot_unavailable");
            }
            let name = contact["name"].as_str().ok_or("carddav_snapshot_unavailable")?;
            let email = contact["email"]
                .as_str()
                .ok_or("carddav_snapshot_unavailable")?;
            if name.len() > MAX_NAME_BYTES
                || name.chars().any(char::is_control)
                || email.len() > MAX_EMAIL_BYTES
                || !crate::contacts::address(email)
            {
                return Err("carddav_snapshot_unavailable");
            }
            contacts.push(json!({"name": name, "email": email}));
            total += 1;
        }
        books.push(json!({"href": href, "contacts": contacts}));
    }
    Ok(books)
}

fn snapshot_write(source_id: &str, books: &Value) -> Result<(), &'static str> {
    let bytes = serde_json::to_vec(&json!({
        "version": 1, "source": source_id, "books": books,
    }))
    .map_err(|_| "carddav_snapshot_unavailable")?;
    if bytes.len() > SNAPSHOT_LIMIT {
        return Err("carddav_snapshot_unavailable");
    }
    let dir = snapshot_dir(true)?.ok_or("carddav_snapshot_unavailable")?;
    crate::platform::private_fs::atomic_replace(&dir, &snapshot_name(source_id), &bytes)
        .map_err(|_| "carddav_snapshot_unavailable")?;
    crate::platform::private_fs::sync_dir(&dir).map_err(|_| "carddav_snapshot_unavailable")
}

fn snapshot_forget(source_id: &str) -> Result<(), &'static str> {
    if let Some(dir) = snapshot_dir(false)? {
        crate::platform::private_fs::remove_owned(&dir, &snapshot_name(source_id))
            .map_err(|_| "carddav_snapshot_unavailable")?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// RPC entry points.

const PRINCIPAL: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:current-user-principal/></d:prop></d:propfind>";
const HOME: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:propfind xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:prop><c:addressbook-home-set/></d:prop></d:propfind>";
const BOOK_PROPS: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><d:propfind xmlns:d=\"DAV:\"><d:prop><d:displayname/><d:resourcetype/></d:prop></d:propfind>";
const ADDRESSBOOK_QUERY: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?><c:addressbook-query xmlns:d=\"DAV:\" xmlns:c=\"urn:ietf:params:xml:ns:carddav\"><d:prop><d:getetag/><c:address-data/></d:prop></c:addressbook-query>";

fn propfind() -> Method {
    Method::from_bytes(b"PROPFIND").unwrap()
}

fn report() -> Method {
    Method::from_bytes(b"REPORT").unwrap()
}

async fn discover_with<F, Fut>(params: &Value, lookup: F) -> Result<Value, &'static str>
where
    F: FnOnce(crate::credentials::CredentialKey) -> Fut,
    Fut: std::future::Future<
        Output = Result<crate::credentials::Secret, crate::credentials::Error>,
    >,
{
    discover_on(client()?, params, lookup).await
}

async fn discover_on<F, Fut>(
    client: &reqwest::Client,
    params: &Value,
    lookup: F,
) -> Result<Value, &'static str>
where
    F: FnOnce(crate::credentials::CredentialKey) -> Fut,
    Fut: std::future::Future<
        Output = Result<crate::credentials::Secret, crate::credentials::Error>,
    >,
{
    let fields = params.as_object().ok_or("invalid_params")?;
    if fields
        .keys()
        .any(|key| !["source", "collection"].contains(&key.as_str()))
    {
        return Err("invalid_params");
    }
    let source = source(params)?;
    let collection = match fields.get("collection") {
        None => None,
        Some(value) => {
            let raw = bounded(value.as_str().ok_or("invalid_params")?, 2048)?;
            // Sharing discovery is not universal; an explicit collection URL
            // still must live on the origin its password belongs to. The
            // destination is pinned before any credential is looked up.
            let target = source_url(raw)?;
            if target.origin() != source.url.origin() {
                return Err("carddav_origin_refused");
            }
            Some(target)
        }
    };
    let secret = password_with(&source.id, lookup).await?;
    let password = secret.text().map_err(|_| "carddav_password_invalid")?;
    let run = async {
        if let Some(target) = collection {
            let (url, xml) = dav_with(
                client,
                target,
                &propfind(),
                "0",
                BOOK_PROPS,
                &source.username,
                password,
                DISCOVERY_LIMIT,
            )
            .await?;
            let books = book_list(&xml, &url)?;
            // A Depth:0 answer about the book itself still parses through the
            // same response shape.
            match books.into_iter().next() {
                Some(book) => return Ok(json!({"books": [book]})),
                None => return Err("carddav_not_addressbook"),
            }
        }
        let (url, xml) = dav_with(
            client,
            source.url.clone(),
            &propfind(),
            "0",
            PRINCIPAL,
            &source.username,
            password,
            DISCOVERY_LIMIT,
        )
        .await?;
        let mut principals = nested_hrefs(&xml, b"current-user-principal")?;
        let Some(principal) = principals.pop() else {
            return Err("carddav_discovery_failed");
        };
        let (url, xml) = dav_with(
            client,
            resolve(&url, &principal)?,
            &propfind(),
            "0",
            HOME,
            &source.username,
            password,
            DISCOVERY_LIMIT,
        )
        .await?;
        let homes = nested_hrefs(&xml, b"addressbook-home-set")?;
        if homes.is_empty() {
            return Err("carddav_discovery_failed");
        }
        let mut books = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for home in homes {
            let (url, xml) = dav_with(
                client,
                resolve(&url, &home)?,
                &propfind(),
                "1",
                BOOK_PROPS,
                &source.username,
                password,
                DISCOVERY_LIMIT,
            )
            .await?;
            for book in book_list(&xml, &url)? {
                if seen.insert(book["href"].as_str().unwrap_or_default().to_owned()) {
                    books.push(book);
                }
            }
        }
        Ok(json!({"books": books}))
    };
    tokio::time::timeout(Duration::from_secs(22), run)
        .await
        .map_err(|_| "carddav_timeout")?
}

async fn refresh_with<F, Fut>(params: &Value, lookup: F) -> Result<Value, &'static str>
where
    F: FnOnce(crate::credentials::CredentialKey) -> Fut,
    Fut: std::future::Future<
        Output = Result<crate::credentials::Secret, crate::credentials::Error>,
    >,
{
    refresh_on(client()?, params, lookup).await
}

async fn refresh_on<F, Fut>(
    client: &reqwest::Client,
    params: &Value,
    lookup: F,
) -> Result<Value, &'static str>
where
    F: FnOnce(crate::credentials::CredentialKey) -> Fut,
    Fut: std::future::Future<
        Output = Result<crate::credentials::Secret, crate::credentials::Error>,
    >,
{
    let fields = params.as_object().ok_or("invalid_params")?;
    if fields.len() != 2 || fields.keys().any(|key| !["source", "books"].contains(&key.as_str()))
    {
        return Err("invalid_params");
    }
    let source = source(params)?;
    let wanted = fields["books"].as_array().ok_or("invalid_params")?;
    if wanted.len() > MAX_BOOKS {
        return Err("invalid_params");
    }
    let mut targets = Vec::new();
    for book in wanted {
        let fields = book.as_object().ok_or("invalid_params")?;
        if fields.len() != 1 || !fields.contains_key("href") {
            return Err("invalid_params");
        }
        let href = bounded(fields["href"].as_str().ok_or("invalid_params")?, 8192)?;
        targets.push(resolve(&source.url, href).map_err(|_| "invalid_params")?);
    }
    let secret = password_with(&source.id, lookup).await?;
    let password = secret.text().map_err(|_| "carddav_password_invalid")?;
    let source_id = source.id.clone();
    let run = async move {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut books = Vec::new();
        let mut errors = Vec::new();
        for target in &targets {
            let result = if std::time::Instant::now() >= deadline {
                Err("carddav_timeout")
            } else {
                match dav_with(
                    client,
                    target.clone(),
                    &report(),
                    "1",
                    ADDRESSBOOK_QUERY,
                    &source.username,
                    password,
                    BOOK_LIMIT,
                )
                .await
                {
                    Ok((_, xml)) => book_contacts(&xml),
                    Err(error) => Err(error),
                }
            };
            match result {
                Ok(contacts) => books.push(json!({"href": target.as_str(), "contacts": contacts})),
                Err(error) => errors.push(json!({"href": target.as_str(), "error": error})),
            }
        }
        // A book that failed keeps whatever the last complete refresh stored;
        // nothing partial replaces it. Books no longer configured are dropped.
        let prior = snapshot_read(&source_id).unwrap_or_else(|_| json!({"books": []}));
        let mut merged: Vec<Value> = prior["books"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|book| {
                let href = book["href"].as_str().unwrap_or_default();
                targets.iter().any(|target| target.as_str() == href)
                    && errors.iter().any(|entry| entry["href"].as_str() == Some(href))
            })
            .collect();
        merged.extend(books.iter().cloned());
        let stored = snapshot_write(&source_id, &json!(merged)).is_ok();
        Ok(json!({"books": books, "errors": errors, "stored": stored}))
    };
    tokio::time::timeout(Duration::from_secs(22), run)
        .await
        .map_err(|_| "carddav_timeout")?
}

fn source_id_param(params: &Value) -> Result<&str, &'static str> {
    let fields = params.as_object().ok_or("invalid_params")?;
    if fields.len() != 1 || !fields.contains_key("sourceId") {
        return Err("invalid_params");
    }
    valid_source_id(bounded(
        fields["sourceId"].as_str().ok_or("invalid_params")?,
        1024,
    )?)
}

fn snapshot(params: &Value) -> Result<Value, &'static str> {
    snapshot_read(source_id_param(params)?)
}

fn forget(params: &Value) -> Result<Value, &'static str> {
    snapshot_forget(source_id_param(params)?)?;
    Ok(json!({"forgotten": true}))
}

pub async fn call(method: &str, params: &Value) -> Result<Value, &'static str> {
    match method {
        "carddav.discover" => discover_with(params, crate::credentials::get).await,
        "carddav.refresh" => refresh_with(params, crate::credentials::get).await,
        "carddav.snapshot" => {
            let params = params.clone();
            tokio::task::spawn_blocking(move || snapshot(&params))
                .await
                .map_err(|_| "worker_failed")?
        }
        "carddav.forget" => {
            let params = params.clone();
            tokio::task::spawn_blocking(move || forget(&params))
                .await
                .map_err(|_| "worker_failed")?
        }
        _ => Err("unknown_method"),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod runtime_tests;
