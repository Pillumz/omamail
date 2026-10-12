use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::SocketAddr;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Arc;

struct Peer {
    process: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    client: reqwest::Client,
}

// No lookup can leave the test, even if a regression accepts a foreign host.
struct LoopbackDns(SocketAddr);
impl reqwest::dns::Resolve for LoopbackDns {
    fn resolve(&self, _: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let address = self.0;
        Box::pin(async move { Ok(Box::new(std::iter::once(address)) as reqwest::dns::Addrs) })
    }
}

impl Peer {
    fn new() -> Self {
        let mut process = Command::new("python3")
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/src/carddav/tls_test.py"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = process.stdin.take().unwrap();
        let mut output = BufReader::new(process.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let info: Value = serde_json::from_str(&line).unwrap();
        let address: SocketAddr = format!("127.0.0.1:{}", info["port"]).parse().unwrap();
        let certificate = reqwest::Certificate::from_pem(
            &std::fs::read(info["certificate"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        let client = client_builder()
            .dns_resolver(Arc::new(LoopbackDns(address)))
            .add_root_certificate(certificate)
            .build()
            .unwrap();
        Self {
            process,
            input,
            output,
            client,
        }
    }

    fn command(&mut self, value: Value) -> Value {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn responses(&mut self, responses: Value) {
        self.command(json!({"responses": responses}));
    }

    // The security property lives at request granularity: which host received
    // what, with which credential. A bare TCP/TLS connection count is pool
    // timing — an idle pooled connection the stub dropped is re-dialed
    // without any request crossing it — so it asserts nothing here.
    fn requests(&mut self) -> Vec<Value> {
        let report = self.command(json!({}));
        report["requests"].as_array().unwrap().clone()
    }
}

impl Drop for Peer {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "{{\"stop\":true}}");
        let _ = self.input.flush();
        let _ = self.process.wait();
    }
}

fn synthetic_lookup(
    _key: crate::credentials::CredentialKey,
) -> impl std::future::Future<Output = Result<crate::credentials::Secret, crate::credentials::Error>>
{
    async { crate::credentials::Secret::new(b"synthetic".to_vec()) }
}

fn missing_lookup(
    _key: crate::credentials::CredentialKey,
) -> impl std::future::Future<Output = Result<crate::credentials::Secret, crate::credentials::Error>>
{
    async { Err(crate::credentials::Error::Missing) }
}

const PRINCIPAL_ANSWER: &str = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/card/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/pal/employee%40example.org/</d:href></d:current-user-principal></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
const HOME_ANSWER: &str = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/dav/pal/employee%40example.org/</d:href><d:propstat><d:prop><c:addressbook-home-set><d:href>/dav/card/employee%40example.org/</d:href></c:addressbook-home-set></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
const BOOKS_ANSWER: &str = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/dav/card/employee%40example.org/</d:href><d:propstat><d:prop><d:displayname>Contacts</d:displayname><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/dav/card/employee%40example.org/default/</d:href><d:propstat><d:prop><d:displayname>My contacts</d:displayname><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><d:displayname/><c:getctag/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response><d:response><d:href>/dav/card/directory%40example.org/cdm-staff/</d:href><d:propstat><d:prop><d:displayname>CDM Staff</d:displayname><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;

const STAFF_BOOK: &str = "https://dav.example.org/dav/card/directory%40example.org/cdm-staff/";
const OWN_BOOK: &str = "https://dav.example.org/dav/card/employee%40example.org/default/";

fn staff_answer() -> String {
    format!(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/dav/card/directory%40example.org/cdm-staff/ivan.vcf</d:href><d:propstat><d:prop><d:getetag>"1"</d:getetag><c:address-data>BEGIN:VCARD
VERSION:4.0
FN:Иван Петров
EMAIL;TYPE=work:ivan@example.org
EMAIL;TYPE=home:ivan.petrov@example.org
PHOTO;VALUE=uri:https://dav.example.org/photos/ivan.jpg
END:VCARD</c:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/dav/card/directory%40example.org/cdm-staff/maria.vcf</d:href><d:propstat><d:prop><d:getetag>"2"</d:getetag><c:address-data>BEGIN:VCARD
VERSION:4.0
FN:Мария Сидорова
EMAIL:maria@example.org
END:VCARD</c:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#
    )
}

fn discover_params() -> Value {
    json!({"source": {
        "id": derive_source_id("https://dav.example.org/dav/card/", "employee@example.org").unwrap(),
        "url": "https://dav.example.org/dav/card/",
        "username": "employee@example.org",
    }})
}

fn refresh_params(hrefs: &[&str]) -> Value {
    let mut params = discover_params();
    params["books"] = json!(hrefs.iter().map(|href| json!({"href": href})).collect::<Vec<_>>());
    params
}

/// A REPORT answer of `cards` contacts whose names sit at the per-contact
/// bound: every book stays inside the per-book limits, and only the
/// aggregate budgets can refuse the total.
fn heavy_book_xml(cards: usize) -> String {
    let name = "n".repeat(MAX_NAME_BYTES);
    let mut xml = String::from(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav">"#,
    );
    for index in 0..cards {
        xml.push_str(&format!(
            "<d:response><d:propstat><d:prop><c:address-data>BEGIN:VCARD\nVERSION:4.0\nFN:{name}\nEMAIL:n{index}@example.org\nEND:VCARD</c:address-data></d:prop></d:propstat></d:response>"
        ));
    }
    xml.push_str("</d:multistatus>");
    xml
}

#[tokio::test]
async fn standard_discovery_finds_own_and_shared_books() {
    let mut peer = Peer::new();
    peer.responses(json!([
        {"status": 207, "body": PRINCIPAL_ANSWER},
        {"status": 207, "body": HOME_ANSWER},
        {"status": 207, "body": BOOKS_ANSWER},
    ]));
    let result = discover_on(&peer.client, &discover_params(), synthetic_lookup)
        .await
        .unwrap();
    let books = result["books"].as_array().unwrap();
    assert_eq!(books.len(), 2);
    assert_eq!(books[0]["href"], OWN_BOOK);
    assert_eq!(books[0]["name"], "My contacts");
    assert_eq!(books[1]["href"], STAFF_BOOK);
    assert_eq!(books[1]["name"], "CDM Staff");
    let requests = peer.requests();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0]["path"], "/dav/card/");
    assert_eq!(requests[1]["path"], "/dav/pal/employee%40example.org/");
    assert_eq!(requests[2]["path"], "/dav/card/employee%40example.org/");
    assert_eq!(requests[2]["depth"], "1");
    for request in &requests {
        assert_eq!(request["method"], "PROPFIND");
        assert_eq!(request["host"], "dav.example.org");
        assert!(
            request["authorization"].as_str().unwrap().starts_with("Basic "),
            "every hop carries the source's own credentials"
        );
    }
}

#[tokio::test]
async fn explicit_collection_skips_principal_discovery() {
    let mut peer = Peer::new();
    let book = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/dav/card/directory%40example.org/cdm-staff/</d:href><d:propstat><d:prop><d:displayname>CDM Staff</d:displayname><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><d:displayname/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response></d:multistatus>"#;
    peer.responses(json!([{"status": 207, "body": book}]));
    let mut params = discover_params();
    params["collection"] = json!(STAFF_BOOK);
    let result = discover_on(&peer.client, &params, synthetic_lookup)
        .await
        .unwrap();
    assert_eq!(result["books"][0]["href"], STAFF_BOOK);
    assert_eq!(result["books"][0]["name"], "CDM Staff");
    let requests = peer.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["path"], "/dav/card/directory%40example.org/cdm-staff/");
    assert_eq!(requests[0]["depth"], "0");

    // The collection must live on the source's origin and be an address book.
    let mut foreign = discover_params();
    foreign["collection"] = json!("https://outside.example.test/card/");
    assert_eq!(
        discover_on(&peer.client, &foreign, synthetic_lookup).await,
        Err("carddav_origin_refused")
    );
    let calendar = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/cal/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    peer.responses(json!([{"status": 207, "body": calendar}]));
    let mut wrong = discover_params();
    wrong["collection"] = json!("https://dav.example.org/dav/cal/");
    assert_eq!(
        discover_on(&peer.client, &wrong, synthetic_lookup).await,
        Err("carddav_not_addressbook")
    );
}

#[tokio::test]
async fn refresh_reads_cards_and_keeps_failed_books_previous_snapshot() {
    let _guard = snapshot_test_lock();
    let root = snapshot_test_root();
    // One book answers; the other refuses. The refusal must not lose the
    // contacts an earlier complete refresh stored.
    let prior_books = json!([{"href": OWN_BOOK, "contacts": [{"name": "Earlier", "email": "earlier@example.org"}]}]);
    snapshot_write(
        &derive_source_id("https://dav.example.org/dav/card/", "employee@example.org").unwrap(),
        prior_books.as_array().unwrap(),
    )
    .unwrap();
    let mut peer = Peer::new();
    peer.responses(json!([
        {"status": 207, "body": staff_answer()},
        {"status": 403},
    ]));
    let result = refresh_on(
        &peer.client,
        &refresh_params(&[STAFF_BOOK, OWN_BOOK]),
        synthetic_lookup,
    )
    .await
    .unwrap();
    assert_eq!(result["stored"], true);
    let books = result["books"].as_array().unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0]["href"], STAFF_BOOK);
    let contacts = books[0]["contacts"].as_array().unwrap();
    assert_eq!(contacts.len(), 3);
    assert_eq!(contacts[0]["name"], "Иван Петров");
    assert_eq!(contacts[0]["email"], "ivan@example.org");
    assert_eq!(contacts[1]["email"], "ivan.petrov@example.org");
    assert_eq!(contacts[2]["name"], "Мария Сидорова");
    let errors = result["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["href"], OWN_BOOK);
    assert_eq!(errors[0]["error"], "carddav_auth_refused");
    // The stored snapshot pairs the fresh book with the retained one.
    let stored = snapshot(&json!({"sourceId": derive_source_id("https://dav.example.org/dav/card/", "employee@example.org").unwrap()})).unwrap();
    let stored_books = stored["books"].as_array().unwrap();
    assert_eq!(stored_books.len(), 2);
    assert_eq!(
        stored_books[0]["contacts"][0]["email"],
        "earlier@example.org"
    );
    assert_eq!(stored_books[1]["contacts"].as_array().unwrap().len(), 3);
    let requests = peer.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(request["method"], "REPORT");
        assert!(request["body"].as_str().unwrap().contains("addressbook-query"));
        assert!(
            !request["body"].as_str().unwrap().contains("PHOTO"),
            "no image or media resource is ever requested"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn aggregate_overflow_is_refused_whole_and_keeps_the_last_snapshot() {
    let _guard = snapshot_test_lock();
    let root = snapshot_test_root();
    let id = derive_source_id("https://dav.example.org/dav/card/", "employee@example.org").unwrap();
    let good = json!([
        {"href": OWN_BOOK, "contacts": [{"name": "Earlier", "email": "earlier@example.org"}]}
    ]);
    snapshot_write(&id, good.as_array().unwrap()).unwrap();

    // Three books, each inside every per-book limit (fewer than MAX_CARDS
    // cards, well under BOOK_LIMIT bytes), whose names make every book
    // serialize to about 8.8 MB: the second already passes the snapshot
    // byte budget that used to be checked only after the whole ~1 GB-scale
    // answer had been accumulated and encoded. The refresh must refuse with
    // the bounded aggregate error, stop fetching the rest, and leave the
    // stored snapshot exactly as it was.
    let hrefs: Vec<String> = (0..3)
        .map(|index| format!("https://dav.example.org/dav/card/heavy{index}/"))
        .collect();
    let refs: Vec<&str> = hrefs.iter().map(|href| href.as_str()).collect();
    let mut peer = Peer::new();
    peer.responses(json!([
        {"status": 207, "body": heavy_book_xml(8_200)},
        {"status": 207, "body": heavy_book_xml(8_200)},
        {"status": 207, "body": heavy_book_xml(8_200)},
    ]));
    assert_eq!(
        refresh_on(&peer.client, &refresh_params(&refs), synthetic_lookup).await,
        Err("carddav_total_too_large"),
        "no giant reply and no stored:false — a resource overflow is a refusal"
    );
    assert_eq!(
        peer.requests().len(),
        2,
        "the loop stops at the book that crosses the budget"
    );
    let stored = snapshot(&json!({"sourceId": id})).unwrap();
    assert_eq!(stored["books"].as_array().unwrap().len(), 1);
    assert_eq!(stored["books"][0]["contacts"][0]["email"], "earlier@example.org");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn retained_books_count_against_the_aggregate_budget() {
    let _guard = snapshot_test_lock();
    let root = snapshot_test_root();
    let id = derive_source_id("https://dav.example.org/dav/card/", "employee@example.org").unwrap();
    let name = "n".repeat(MAX_NAME_BYTES);
    let heavy = |start: usize| {
        (start..start + 7_000)
            .map(|index| json!({"name": name, "email": format!("n{index}@example.org")}))
            .collect::<Vec<_>>()
    };
    // A stored snapshot near the byte cap: two retained books totalling
    // about 15 MB, each inside the per-book contact limit.
    let prior = json!([
        {"href": "https://dav.example.org/dav/card/a/", "contacts": heavy(0)},
        {"href": "https://dav.example.org/dav/card/b/", "contacts": heavy(7_000)},
    ]);
    snapshot_write(&id, prior.as_array().unwrap()).unwrap();

    // Both stored books now fail and must be retained; the fresh book would
    // fit alone, but not beside them. The whole refresh refuses instead of
    // writing a snapshot past the cap or losing the retained contacts.
    let mut peer = Peer::new();
    peer.responses(json!([
        {"status": 403},
        {"status": 403},
        {"status": 207, "body": heavy_book_xml(3_000)},
    ]));
    assert_eq!(
        refresh_on(
            &peer.client,
            &refresh_params(&[
                "https://dav.example.org/dav/card/a/",
                "https://dav.example.org/dav/card/b/",
                "https://dav.example.org/dav/card/c/",
            ]),
            synthetic_lookup
        )
        .await,
        Err("carddav_total_too_large")
    );
    let stored = snapshot(&json!({"sourceId": id})).unwrap();
    let books = stored["books"].as_array().unwrap();
    assert_eq!(books.len(), 2, "the retained books survive untouched");
    assert_eq!(books[0]["contacts"].as_array().unwrap().len(), 7_000);
    assert_eq!(books[1]["contacts"].as_array().unwrap().len(), 7_000);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn https_redirects_never_send_credentials_to_foreign_origins() {
    let mut peer = Peer::new();
    for status in [301, 302, 303, 307, 308] {
        for location in [
            "https://outside.example.test/stolen",
            "//outside.example.test/stolen",
            "http://outside.example.test/stolen",
            "https://dav.example.org.outside.example.test/stolen",
            "https://dav.example.org@outside.example.test/stolen",
            "https://dav.example.org/stolen#fragment",
        ] {
            peer.responses(json!([{"status": status, "location": location}, {"status": 207, "body": "stolen"}]));
            assert!(
                discover_on(&peer.client, &discover_params(), synthetic_lookup)
                    .await
                    .is_err(),
                "accepted {status}: {location}"
            );
            let requests = peer.requests();
            eprintln!("case {status} {location} -> requests {:?}", requests);
            assert_eq!(requests.len(), 1, "followed {status}: {location}: {requests:?}");
            assert_eq!(requests[0]["host"], "dav.example.org");
            assert_eq!(requests[0]["method"], "PROPFIND");
        }
    }
    // Positive control: a same-origin redirect is followed with the same
    // method, body and credential.
    peer.responses(json!([
        {"status": 308, "location": "/dav/card/"},
        {"status": 207, "body": PRINCIPAL_ANSWER},
        {"status": 207, "body": HOME_ANSWER},
        {"status": 207, "body": BOOKS_ANSWER},
    ]));
    let result = discover_on(&peer.client, &discover_params(), synthetic_lookup)
        .await
        .unwrap();
    assert_eq!(result["books"].as_array().unwrap().len(), 2);
    let requests = peer.requests();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[1]["path"], "/dav/card/");
    assert_eq!(requests[1]["method"], "PROPFIND");
    assert!(requests[1]["authorization"].as_str().unwrap().starts_with("Basic "));
}

#[tokio::test]
async fn failures_are_refusals_not_silent_answers() {
    let mut peer = Peer::new();
    // A missing keyring password is reported before any request is made.
    assert_eq!(
        discover_on(&peer.client, &discover_params(), missing_lookup).await,
        Err("carddav_password_missing")
    );
    assert!(peer.requests().is_empty());
    peer.responses(json!([{"status": 401}]));
    assert_eq!(
        discover_on(&peer.client, &discover_params(), synthetic_lookup).await,
        Err("carddav_auth_refused")
    );
    peer.responses(json!([{"status": 207, "body": "<d:multistatus"}]));
    assert_eq!(
        discover_on(&peer.client, &discover_params(), synthetic_lookup).await,
        Err("carddav_invalid_response")
    );
    peer.responses(json!([{"status": 207, "body": PRINCIPAL_ANSWER.replace("current-user-principal", "unrelated")}]));
    assert_eq!(
        discover_on(&peer.client, &discover_params(), synthetic_lookup).await,
        Err("carddav_discovery_failed")
    );
    // An oversized answer is refused whole, not parsed as far as it went.
    peer.responses(json!([{"status": 207, "body": "x".repeat(DISCOVERY_LIMIT + 1)}]));
    assert_eq!(
        discover_on(&peer.client, &discover_params(), synthetic_lookup).await,
        Err("carddav_response_too_large")
    );
}
