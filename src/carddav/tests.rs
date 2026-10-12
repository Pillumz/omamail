use super::*;

fn source_params(url: &str) -> Value {
    // Invalid URLs derive no id; the placeholder keeps the rejection inside
    // source() rather than in the test helper.
    let id = derive_source_id(url, "employee@example.org")
        .unwrap_or_else(|| format!("carddav:{}", "0".repeat(64)));
    json!({"source": {"id": id, "url": url, "username": "employee@example.org"}})
}

#[test]
fn source_id_commits_to_the_canonical_endpoint_and_username() {
    // Known-answer vector; `CarddavSources.deriveId` must produce the same.
    assert_eq!(
        derive_source_id("https://dav.example.org/dav/card/", "employee@example.org").unwrap(),
        "carddav:9ff5d0d63e48ae4dd33ec08f3adbaa748bd131b7f320641e9438adf4c7e8f119"
    );
    // Spellings of one endpoint share an identity, and with it one keyring
    // entry and one snapshot.
    let base = derive_source_id("https://dav.example.org/dav/card/", "employee@example.org").unwrap();
    for spelling in [
        "https://dav.example.org:443/dav/card/",
        "HTTPS://Dav.Example.ORG/dav/card/",
        "  https://dav.example.org/dav/card/  ",
    ] {
        assert_eq!(derive_source_id(spelling, "employee@example.org").unwrap(), base, "{spelling}");
    }
    // Spellings that slug-collide but name different endpoints — or the same
    // endpoint under another account — must never share a credential.
    let collisions = [
        derive_source_id("https://a.b/", "ann").unwrap(),
        derive_source_id("https://a-b/", "ann").unwrap(),
        derive_source_id("https://dav.example.org/dav/card/", "anna").unwrap(),
        derive_source_id("https://dav.example.org:4443/dav/card/", "employee@example.org").unwrap(),
        derive_source_id("https://dav.example.org/dav/other/", "employee@example.org").unwrap(),
    ];
    for (i, a) in collisions.iter().enumerate() {
        for b in &collisions[i + 1..] {
            assert_ne!(a, b);
        }
    }
    assert!(derive_source_id("http://dav.example.org/", "ann").is_none());
}

#[test]
fn source_params_bind_the_id_to_the_url_and_username() {
    assert!(source(&source_params("https://dav.example.org/dav/card/")).is_ok());
    // The id of one endpoint must not be presented with another's URL: that
    // is what would send a stored password somewhere it does not belong.
    let mut forged = source_params("https://dav.example.org/dav/card/");
    forged["source"]["id"] = json!(derive_source_id("https://other.example.org/", "employee@example.org").unwrap());
    assert!(matches!(source(&forged), Err("invalid_params")));
    // The id of one username must not be presented with another's.
    let mut other_user = source_params("https://dav.example.org/dav/card/");
    other_user["source"]["username"] = json!("anna");
    assert!(matches!(source(&other_user), Err("invalid_params")));
    let mut foreign = source_params("https://dav.example.org/dav/card/");
    foreign["source"]["id"] = json!("imap:a@example.org");
    assert!(matches!(source(&foreign), Err("invalid_params")));
    for url in [
        "http://dav.example.org/",
        "https://dav.example.org/#frag",
        "https://dav.example.org/?q=1",
        "https://user@dav.example.org/",
        "https://user:pw@dav.example.org/",
        "dav.example.org",
        "https://dav.example.org/\u{85}",
        "https://dav.example.org\\@evil.example/",
    ] {
        let params = source_params(url);
        assert!(source(&params).is_err(), "{url}");
    }
    let mut extra = source_params("https://dav.example.org/");
    extra["source"]["token"] = json!("x");
    assert!(matches!(source(&extra), Err("invalid_params")));
    let mut space = source_params("https://dav.example.org/");
    space["source"]["username"] = json!("bad name");
    assert!(matches!(source(&space), Err("invalid_params")));
}

#[test]
fn hrefs_must_stay_on_the_passwords_origin() {
    let base = Url::parse("https://dav.example.org/dav/card/employee%40example.org/").unwrap();
    assert_eq!(
        resolve(&base, "book/").unwrap().as_str(),
        "https://dav.example.org/dav/card/employee%40example.org/book/"
    );
    // A shared book outside the home subtree is fine; the origin is the rule.
    assert_eq!(
        resolve(&base, "/dav/card/directory%40example.org/staff/")
            .unwrap()
            .as_str(),
        "https://dav.example.org/dav/card/directory%40example.org/staff/"
    );
    assert_eq!(
        resolve(&base, "https://dav.example.org:443/dav/card/a/").unwrap().host_str(),
        Some("dav.example.org")
    );
    for href in [
        "https://other.example.org/dav/card/a/",
        "//other.example.org/dav/card/a/",
        "http://dav.example.org/dav/card/a/",
        "https://dav.example.org:4443/dav/card/a/",
        "https://dav.example.org.evil.example/",
        "https://user@dav.example.org/",
        "https://dav.example.org/a#frag",
        "https://dav.example.org/a\\b",
        "",
        "relative\nnewline",
    ] {
        assert!(resolve(&base, href).is_err(), "{href}");
    }
}

#[test]
fn principal_and_home_hrefs_are_scoped_to_their_property() {
    let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/card/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/dav/pal/employee%40example.org/</d:href></d:current-user-principal><d:addressbook-home-set xmlns:c="urn:ietf:params:xml:ns:carddav"><d:href>/dav/card/employee%40example.org/</d:href></d:addressbook-home-set></d:prop></d:propstat></d:response></d:multistatus>"#;
    assert_eq!(
        nested_hrefs(xml, b"current-user-principal").unwrap(),
        ["/dav/pal/employee%40example.org/"]
    );
    assert_eq!(
        nested_hrefs(xml, b"addressbook-home-set").unwrap(),
        ["/dav/card/employee%40example.org/"]
    );
}

#[test]
fn doctype_and_foreign_entities_are_refused() {
    let doctype = r#"<?xml version="1.0"?><!DOCTYPE d:multistatus [<!ENTITY xxe SYSTEM "file:///etc/passwd">]><d:multistatus xmlns:d="DAV:"><d:response><d:href>/a/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>&xxe;</d:href></d:current-user-principal></d:prop></d:propstat></d:response></d:multistatus>"#;
    assert!(nested_hrefs(doctype, b"current-user-principal").is_err());
    let entity = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop><d:current-user-principal><d:href>&madeup;</d:href></d:current-user-principal></d:prop></d:propstat></d:response></d:multistatus>"#;
    assert!(nested_hrefs(entity, b"current-user-principal").is_err());
    let escaped = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop><d:current-user-principal><d:href>/dav/pal/a&#37;40b/</d:href></d:current-user-principal></d:prop></d:propstat></d:response></d:multistatus>"#;
    assert_eq!(
        nested_hrefs(escaped, b"current-user-principal").unwrap(),
        ["/dav/pal/a%40b/"]
    );
}

#[test]
fn book_list_keeps_addressbooks_from_split_propstats() {
    // Stalwart answers absent properties in a second 404 propstat; its empty
    // displayname element must not erase the real name.
    let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/dav/card/employee%40example.org/</d:href><d:propstat><d:prop><d:displayname>Home</d:displayname><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/dav/card/employee%40example.org/default/</d:href><d:propstat><d:prop><d:displayname>My contacts</d:displayname><d:resourcetype><d:collection/><c:addressbook/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><d:displayname/><c:getctag/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response><d:response><d:href>/dav/card/directory%40example.org/cdm-staff/</d:href><d:propstat><d:prop><d:displayname>CDM &amp; Staff</d:displayname><d:resourcetype><c:addressbook/><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
    let base = Url::parse("https://dav.example.org/dav/card/employee%40example.org/").unwrap();
    let books = book_list(xml, &base).unwrap();
    assert_eq!(books.len(), 2);
    assert_eq!(books[0]["name"], "My contacts");
    assert_eq!(
        books[0]["href"],
        "https://dav.example.org/dav/card/employee%40example.org/default/"
    );
    // The shared book lives outside the user's home subtree; same origin is
    // what matters.
    assert_eq!(books[1]["name"], "CDM & Staff");
    assert_eq!(
        books[1]["href"],
        "https://dav.example.org/dav/card/directory%40example.org/cdm-staff/"
    );
    // A book href that points off-origin fails the whole answer.
    let foreign = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>https://evil.example/card/</d:href><d:propstat><d:prop><d:resourcetype><c:addressbook/></d:resourcetype></d:prop></d:propstat></d:response></d:multistatus>"#;
    assert!(matches!(
        book_list(foreign, &base),
        Err("carddav_origin_refused")
    ));
}

fn card(card: &str) -> Vec<Value> {
    let xml = format!(
        r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/card.vcf</d:href><d:propstat><d:prop><d:getetag>"1"</d:getetag><c:address-data>{}</c:address-data></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#,
        card
    );
    book_contacts(&xml).unwrap()
}

#[test]
fn vcards_parse_names_addresses_folding_and_escapes() {
    let contacts = card(
        "BEGIN:VCARD\r\nVERSION:4.0\r\nFN:Иван \\, Петров\r\nEMAIL;TYPE=work:ivan@example.org\r\nEMAIL;TYPE=home;PREF=1:ivan.petrov@example.org\r\nEND:VCARD\r\n",
    );
    assert_eq!(
        json!(contacts),
        json!([
            {"name": "Иван , Петров", "email": "ivan@example.org"},
            {"name": "Иван , Петров", "email": "ivan.petrov@example.org"}
        ])
    );
    // A folded value continues on the next line after one space or tab.
    let folded = card("BEGIN:VCARD\nVERSION:3.0\nFN:A very long name that a server\n\tfolds onto a second line\nEMAIL:a@example.org\nEND:VCARD\n");
    assert_eq!(folded[0]["name"], "A very long name that a serverfolds onto a second line");
}

#[test]
fn v3_quoted_printable_and_charset_decode() {
    let contacts = card(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=D0=98=D0=B2=D0=B0=D0=BD\r\nEMAIL:ivan@example.org\r\nEND:VCARD\r\n",
    );
    assert_eq!(contacts[0]["name"], "Иван");
    let cyrillic = card(
        "BEGIN:VCARD\nVERSION:3.0\nFN;CHARSET=WINDOWS-1251;ENCODING=QUOTED-PRINTABLE:=C8=E2=E0=ED\nEMAIL:w@example.org\nEND:VCARD\n",
    );
    assert_eq!(cyrillic[0]["name"], "Иван");
    // A quoted-printable value folded at a soft break: the '=' ends the
    // first physical line and the fold's whitespace follows. Both disappear.
    let soft = card(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=D0=9F=D0=B5=D1=82=D1=80=D0=BE=D0=B2=\r\n =D0=98=D0=B2=D0=B0=D0=BD\r\nEMAIL:ivan@example.org\r\nEND:VCARD\r\n",
    );
    assert_eq!(soft[0]["name"], "ПетровИван");
    // A fold between the hex pairs of one encoded character is a plain join.
    let between = card(
        "BEGIN:VCARD\r\nVERSION:3.0\r\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=D0=9F=D0\r\n =B5=D1=82=D1=80=D0=BE=D0=B2\r\nEMAIL:ivan@example.org\r\nEND:VCARD\r\n",
    );
    assert_eq!(between[0]["name"], "Петров");
    // A plain value whose fold happens to follow a literal '=' keeps it.
    let plain = card("BEGIN:VCARD\nVERSION:4.0\nFN:formula a=\n b\nEMAIL:a@example.org\nEND:VCARD\n");
    assert_eq!(plain[0]["name"], "formula a=b");
}

#[test]
fn parsed_fields_stay_inside_the_snapshot_bounds() {
    let long_name = card(&format!(
        "BEGIN:VCARD\nFN:{}\nEMAIL:a@example.org\nEND:VCARD\n",
        "n".repeat(MAX_NAME_BYTES + 500)
    ));
    assert_eq!(long_name[0]["name"].as_str().unwrap().len(), MAX_NAME_BYTES);
    // A name that passes parsing must also pass a snapshot read-back.
    let books = json!([{"href": "https://dav.example.org/b/", "contacts": long_name}]);
    let parsed = snapshot_books(&json!({"version": 1, "books": books})).unwrap();
    assert_eq!(parsed[0]["contacts"][0]["name"].as_str().unwrap().len(), MAX_NAME_BYTES);
    let long_email = card(&format!(
        "BEGIN:VCARD\nFN:N\nEMAIL:{}@example.org\nEND:VCARD\n",
        "e".repeat(MAX_EMAIL_BYTES)
    ));
    assert_eq!(json!(long_email), json!([]));
    // A multibyte name is truncated on a character boundary, never mid-UTF-8.
    let wide = card(&format!(
        "BEGIN:VCARD\nFN:{}\nEMAIL:a@example.org\nEND:VCARD\n",
        "П".repeat(MAX_NAME_BYTES)
    ));
    let name = wide[0]["name"].as_str().unwrap();
    assert!(name.len() <= MAX_NAME_BYTES && name.chars().all(|c| c == 'П'));
}

#[test]
fn empty_address_data_and_bad_addresses_yield_no_contacts() {
    let xml = r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:href>/a.vcf</d:href><d:propstat><d:prop><d:getetag>"1"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat><d:propstat><d:prop><c:address-data/></d:prop><d:status>HTTP/1.1 404 Not Found</d:status></d:propstat></d:response></d:multistatus>"#;
    assert_eq!(json!(book_contacts(xml).unwrap()), json!([]));
    let contacts = card("BEGIN:VCARD\nFN:No Address\nEMAIL:not-an-address\nEND:VCARD\n");
    assert_eq!(json!(contacts), json!([]));
    // The server's address-data text is escaped like any XML text.
    let escaped = book_contacts(r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:response><d:propstat><d:prop><c:address-data>BEGIN:VCARD
FN:Fish &amp; Chips
EMAIL:fish@example.org
END:VCARD</c:address-data></d:prop></d:propstat></d:response></d:multistatus>"#).unwrap();
    assert_eq!(escaped[0]["name"], "Fish & Chips");
}

#[test]
fn oversized_books_are_refused_not_truncated() {
    let mut xml = String::from(r#"<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav">"#);
    for index in 0..MAX_CARDS + 1 {
        xml.push_str(&format!("<d:response><d:propstat><d:prop><c:address-data>BEGIN:VCARD\nFN:N\nEMAIL:n{index}@example.org\nEND:VCARD</c:address-data></d:prop></d:propstat></d:response>"));
    }
    xml.push_str("</d:multistatus>");
    assert!(matches!(book_contacts(&xml), Err("carddav_too_many_contacts")));
}

#[test]
fn oversize_snapshots_are_refused_whole_by_a_bounded_encoding() {
    let _guard = snapshot_test_lock();
    let root = snapshot_test_root();
    let id = derive_source_id("https://dav.example.org/", "ann").unwrap();
    // Every contact is valid and the per-book limits never ran; only the
    // whole file passes the snapshot cap. The encoder must refuse mid-way
    // with the aggregate error and leave no file behind — a last good
    // answer cannot be replaced by a refused one.
    let name = "n".repeat(MAX_NAME_BYTES);
    let contacts: Vec<Value> = (0..SNAPSHOT_LIMIT / MAX_NAME_BYTES + 2)
        .map(|index| json!({"name": name, "email": format!("n{index}@example.org")}))
        .collect();
    let books = json!([{"href": "https://dav.example.org/big/", "contacts": contacts}]);
    assert!(matches!(
        snapshot_write(&id, books.as_array().unwrap()),
        Err("carddav_total_too_large")
    ));
    assert_eq!(snapshot_read(&id).unwrap(), json!({"books": []}));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn colliding_spellings_keep_separate_credentials_and_snapshots() {
    let _guard = snapshot_test_lock();
    let _root = snapshot_test_root();
    // Slug-style ids would merge these; the hashed canonical identity must not.
    let a = derive_source_id("https://a.b/", "ann").unwrap();
    let b = derive_source_id("https://a-b/", "ann").unwrap();
    assert_ne!(a, b);
    assert_ne!(snapshot_name(&a), snapshot_name(&b));
    for (id, expected) in [(a.clone(), a.clone()), (b.clone(), b.clone())] {
        let seen = std::cell::RefCell::new(String::new());
        let _ = password_with(&id, |key| {
            seen.replace(key.account_id.clone());
            async { Err(crate::credentials::Error::Missing) }
        })
        .await;
        assert_eq!(seen.into_inner(), expected);
    }
    let seeded = json!([{"href": "https://a.b/x/", "contacts": []}]);
    snapshot_write(&a, seeded.as_array().unwrap()).unwrap();
    assert_eq!(json!(snapshot_read(&b).unwrap()), json!({"books": []}));
    assert_eq!(snapshot_read(&a).unwrap()["books"].as_array().unwrap().len(), 1);
}

#[test]
fn snapshots_round_trip_and_validate() {
    let books = json!([{"href":"https://dav.example.org/book/","contacts":[{"name":"Иван","email":"ivan@example.org"}]}]);
    let raw = json!({"version":1,"source":"carddav:9ff5d0d6","books":books});
    let parsed = snapshot_books(&raw).unwrap();
    assert_eq!(parsed[0]["contacts"][0]["name"], "Иван");
    for bad in [
        json!({"version":2,"books":[]}),
        json!({"version":1,"books":[{}]}),
        json!({"version":1,"books":[{"href":"x","contacts":[{"name":"N","email":"not an address"}]}]}),
        json!({"version":1,"books":[{"href":"x","contacts":[{"name":"control\n","email":"a@example.org"}]}]}),
        json!({"version":1,"books":[{"href":"x","contacts":[{"name":"N","email":"e".repeat(MAX_EMAIL_BYTES + 1) + "@example.org"}]}]}),
        json!({"version":1,"books":[{"href":"x","contacts":[{"name":"n".repeat(MAX_NAME_BYTES + 1),"email":"a@example.org"}]}]}),
        json!({"version":1,"books":[{"href":"x","contacts":"unbounded"}]}),
    ] {
        assert!(snapshot_books(&bad).is_err(), "{bad}");
    }
}
