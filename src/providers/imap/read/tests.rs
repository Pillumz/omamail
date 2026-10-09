use super::*;
/// Expand a wire sequence set (`1:3,7`) into exactly the UIDs it names.
fn expand(set: &str) -> Vec<u32> {
    set.split(',')
        .flat_map(|token| match token.split_once(':') {
            Some((first, last)) => {
                first.parse::<u32>().unwrap()..=last.parse::<u32>().unwrap()
            }
            None => {
                let uid = token.parse::<u32>().unwrap();
                uid..=uid
            }
        })
        .collect()
}
#[test]
fn uid_batches_frame_adjacent_ranges_within_command_and_response_bounds() {
    let dense: Vec<u32> = (1..=9000).collect();
    let batches = uid_batches(&dense, 4096);
    assert_eq!(
        batches
            .iter()
            .map(|(_, set)| set.as_str())
            .collect::<Vec<_>>(),
        ["1:4096", "4097:8192", "8193:9000"]
    );
    assert_eq!(uid_batches(&[5, 6, 7, 9], 4096)[0].1, "5:7,9");
    // Sparse ten-digit UIDs chunk by command bytes; no range may span a gap.
    let sparse: Vec<u32> = (0..1000).map(|i| 3_000_000_000 + i * 1000).collect();
    let batches = uid_batches(&sparse, 4096);
    assert!(batches.len() >= 2);
    for (window, set) in &batches {
        assert!(set.len() <= UID_SET_BYTES);
        assert!(window.len() <= 4096);
        assert_eq!(&expand(set), window, "a set must name exactly its batch");
    }
    assert_eq!(
        batches
            .iter()
            .flat_map(|(window, _)| window.iter().copied())
            .collect::<Vec<_>>(),
        sparse
    );
    // Adjacency at the u32::MAX edge must not overflow.
    assert_eq!(
        uid_batches(&[u32::MAX - 1, u32::MAX], 4096)[0].1,
        "4294967294:4294967295"
    );
    assert_eq!(uid_batches(&[u32::MAX], 4096)[0].1, "4294967295");
    assert!(uid_batches(&[], 4096).is_empty());
}
#[test]
fn octet_literals_do_not_create_responses_or_fetch_fields() {
    let raw = b"Subject: test\r\n\r\n* 8 FETCH (UID 999)\r\n\xc3\xa9";
    let data=[format!("* 1 FETCH (UID 42 FLAGS (\\Seen) INTERNALDATE \"11-Sep-2026 12:00:00 +0000\" RFC822.SIZE 100 BODY[] {{{}}}\r\n",raw.len()).as_bytes(),raw,b")\r\nO1 OK done\r\n"].concat();
    let boxes = parse_folders(b"* LIST (\\Inbox) \"/\" INBOX\r\n").unwrap();
    let parsed = parse_messages(&data, "INBOX", true, &boxes).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["id"], "42:INBOX");
    assert_eq!(parsed[0]["labelIds"], json!(["INBOX"]));
    assert_eq!(parsed[0]["payload"]["headers"][0]["value"], "test");
    assert_eq!(
        fetched_dates(&data)
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [42]
    );
}
#[test]
fn list_handles_literals_noselect_special_use_and_modified_utf7() {
    let boxes=parse_folders(b"* CAPABILITY IMAP4rev1 ID MOVE\r\n* LIST (\\Noselect) \"/\" Root\r\n* LIST (\\Sent) \"/\" {10}\r\nSent Items\r\n* LIST () NIL &ZeVnLIqe-\r\n").unwrap();
    assert_eq!(resolve(&boxes, "\\Sent").unwrap(), "Sent Items");
    assert_eq!(resolve(&boxes, "\\Trash"), Err("imap_folder_unavailable"));
    let value = folders_value(&boxes);
    assert_eq!(value["labels"].as_array().unwrap().len(), 2);
    assert_eq!(value["labels"][1]["name"], "日本語");
}
#[test]
fn date_pages_are_descending_unique_with_uid_tiebreaker() {
    let dates = BTreeMap::from([(4, 100), (7, 200), (99, 300)]);
    let result = page(&[7, 4, 7, 99], &dates, "INBOX", 1, 2, true);
    assert_eq!(result["ids"], json!(["7:INBOX", "4:INBOX"]));
    assert_eq!(result["nextPageToken"], "3");
    assert_eq!(result["estimate"], 4);
    let dates = fetched_dates(b"* 1 FETCH (UID 1 INTERNALDATE \" 1-Sep-2026 12:00:00 +0200\")\r\n* 2 FETCH (INTERNALDATE \"01-Sep-2026 10:00:00 +0000\" UID 2)\r\n* 3 FETCH (UID 3 INTERNALDATE \"invalid\")\r\n").unwrap();
    assert_eq!(
        page(&[1, 2, 3, 4], &dates, "INBOX", 0, 10, false)["ids"],
        json!(["2:INBOX", "1:INBOX", "4:INBOX", "3:INBOX"])
    );
    assert_eq!(
        query("folder:\"Sent Items\" UNSEEN").unwrap(),
        ("Sent Items".into(), "UNSEEN".into())
    );
    assert!(query("folder:INBOX ALL\r\nEXPUNGE").is_err());
    assert_eq!(
        search_uids(b"* SEARCH 10 7 10\r\nO1 OK done\r\n").unwrap(),
        [7, 10]
    );
}
#[test]
fn parser_bounds_nesting_and_truncated_literals() {
    assert!(nodes(&[b'('; 100]).is_err());
    assert!(nodes(b"* LIST () NIL {20}\r\nshort").is_err());
}
async fn greeting(w: &mut Wire) {
    write(w, b"* OK ready\r\n").await.unwrap();
    assert!(line(w).await.unwrap().starts_with(b"O1 LOGIN"));
    write(w, b"O1 OK login\r\n").await.unwrap();
    for _ in 0..2 {
        assert_eq!(line(w).await.unwrap(), b"O1 CAPABILITY\r\n");
        write(w, b"* CAPABILITY IMAP4rev1\r\nO1 OK caps\r\n")
            .await
            .unwrap();
    }
    assert_eq!(line(w).await.unwrap(), b"O1 LIST \"\" \"*\"\r\n");
    write(w, b"* LIST () \"/\" INBOX\r\nO1 OK folders\r\n")
        .await
        .unwrap();
}
async fn select(w: &mut Wire) {
    assert_eq!(line(w).await.unwrap(), b"O1 SELECT \"INBOX\"\r\n");
    write(w, b"O1 OK selected\r\n").await.unwrap();
}
fn params(port: u16) -> Value {
    json!({"settings":{"imapHost":"127.0.0.1","imapPort":port,"username":"synthetic","insecure":true,"testPlaintext":true},"credential":"synthetic:secret","oauth":false,"query":"folder:INBOX UNSEEN","limit":3,"progressive":true,"requestToken":"request-1"})
}
#[tokio::test]
async fn sparse_search_orders_by_date_before_paging_even_when_progressive() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut w: Wire = BufReader::new(Box::new(stream));
        greeting(&mut w).await;
        select(&mut w).await;
        for page_index in 0..2 {
            assert_eq!(line(&mut w).await.unwrap(), b"O1 UID FETCH 1:* (UID)\r\n");
            write(&mut w, b"* 1 FETCH (UID 7)\r\n* 2 FETCH (UID 1000)\r\n* 3 FETCH (UID 50000)\r\nO1 OK snapshot\r\n").await.unwrap();
            assert_eq!(
                line(&mut w).await.unwrap(),
                b"O1 UID FETCH 7,1000,50000 (UID INTERNALDATE)\r\n"
            );
            write(&mut w,b"* 1 FETCH (UID 7 INTERNALDATE \"23-Sep-2026 12:00:00 +0000\")\r\n* 2 FETCH (UID 1000 INTERNALDATE \"22-Sep-2026 12:00:00 +0000\")\r\n* 3 FETCH (UID 50000 INTERNALDATE \"21-Sep-2026 12:00:00 +0000\")\r\nO1 OK snapshot\r\n").await.unwrap();
            assert_eq!(
                line(&mut w).await.unwrap(),
                b"O1 UID SEARCH UID 7:50000 UNSEEN\r\n"
            );
            write(&mut w, b"* SEARCH 7 1000 50000\r\nO1 OK found\r\n")
                .await
                .unwrap();
            if page_index == 0 {
                select(&mut w).await;
            }
        }
    });
    let mut p = params(port);
    p["limit"] = json!(2);
    let first = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(first["page"]["ids"], json!(["7:INBOX", "1000:INBOX"]));
    assert!(first["continuation"].is_null());
    p["pageToken"] = first["page"]["nextPageToken"].clone();
    let second = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(second["page"]["ids"], json!(["50000:INBOX"]));
    assert_eq!(second["page"]["nextPageToken"], "");
    peer.await.unwrap();
}
#[test]
fn unsolicited_flags_preserve_snapshot_dates() {
    let dates = fetched_dates(b"* 1 FETCH (UID 7 INTERNALDATE \"23-Sep-2026 12:00:00 +0000\")\r\n* 2 FETCH (UID 8 FLAGS ())\r\n* 2 FETCH (UID 8 INTERNALDATE \"22-Sep-2026 12:00:00 +0000\")\r\n* 1 FETCH (UID 7 FLAGS (\\Seen))\r\n").unwrap();
    assert_eq!(
        page(&[7, 8], &dates, "INBOX", 0, 1, false)["ids"],
        json!(["7:INBOX"])
    );
}

#[tokio::test]
async fn multi_window_dates_settle_before_paging_and_ignore_unsolicited_flags() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut w: Wire = BufReader::new(Box::new(stream));
        greeting(&mut w).await;
        let mut searches = 0;
        let mut batches = Vec::new();
        loop {
            let request = String::from_utf8(line(&mut w).await.unwrap()).unwrap();
            let mut response = String::new();
            if request == "O1 SELECT \"INBOX\"\r\n" {
                response.push_str("O1 OK selected\r\n");
            } else if let Some(fetch) = request.strip_prefix("O1 UID FETCH ") {
                let (set, fields) = fetch.split_once(' ').unwrap();
                let ids: Vec<u32> = if set == "1:*" {
                    (1..=4100).collect()
                } else {
                    expand(set)
                };
                let dated = fields.contains("INTERNALDATE");
                if dated {
                    assert!(ids.len() <= 4096, "date response must be bounded");
                    batches.push(ids.len());
                }
                for uid in ids {
                    // Date order deliberately crosses the 4096-message boundary.
                    let day = match uid {
                        1 => 24,
                        4097 => 23,
                        2 => 22,
                        _ => 21,
                    };
                    response.push_str(&if dated {
                        format!("* {uid} FETCH (UID {uid} INTERNALDATE \"{day}-Sep-2026 12:00:00 +0000\")\r\n")
                    } else {
                        format!("* {uid} FETCH (UID {uid})\r\n")
                    });
                }
                // Both within and outside the current batch: neither may erase
                // UID 1's date or add a post-snapshot arrival to the page.
                if dated {
                    response.push_str(
                        "* 1 FETCH (UID 1 FLAGS (\\Seen))\r\n* 4101 FETCH (UID 4101 FLAGS ())\r\n",
                    );
                }
                response.push_str("O1 OK fetched\r\n");
            } else if let Some(search) = request.strip_prefix("O1 UID SEARCH UID ") {
                let (range, _) = search.split_once(' ').unwrap();
                let (first, last) = range.split_once(':').unwrap();
                let first: u32 = first.parse().unwrap();
                let last: u32 = last.parse().unwrap();
                response.push_str("* SEARCH");
                for uid in first..=last.min(4100) {
                    response.push_str(&format!(" {uid}"));
                }
                response.push_str("\r\nO1 OK searched\r\n");
                searches += 1;
            } else {
                panic!("unexpected command: {request}");
            }
            write(&mut w, response.as_bytes()).await.unwrap();
            if searches == 4 {
                break;
            }
        }
        assert_eq!(batches, [4096, 4, 4096, 4]);
    });
    let mut p = params(port);
    p["limit"] = json!(2);
    // Also usable as a runtime regression on the old UID-only implementation:
    // it understands the same snapshot/search commands but chooses the wrong page.
    p["progressive"] = json!(false);
    let first = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(first["page"]["ids"], json!(["1:INBOX", "4097:INBOX"]));
    assert!(first["continuation"].is_null());
    p["pageToken"] = first["page"]["nextPageToken"].clone();
    let second = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(second["page"]["ids"], json!(["2:INBOX", "4100:INBOX"]));
    peer.await.unwrap();
}

#[tokio::test]
async fn native_metadata_fetch_and_mime_parse_stay_in_backend() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut w: Wire = BufReader::new(Box::new(stream));
        greeting(&mut w).await;
        select(&mut w).await;
        assert_eq!(line(&mut w).await.unwrap(),b"O1 UID FETCH 7,8 (UID FLAGS INTERNALDATE RFC822.SIZE BODY.PEEK[HEADER.FIELDS (FROM TO CC SUBJECT DATE MESSAGE-ID REPLY-TO LIST-UNSUBSCRIBE)])\r\n");
        for uid in [8, 7] {
            let raw = format!("From: Test <test@example.org>\r\nSubject: Message {uid}\r\n\r\n");
            write(&mut w,format!("* {uid} FETCH (UID {uid} FLAGS (\\Flagged) RFC822.SIZE 88 BODY[HEADER.FIELDS (FROM SUBJECT)] {{{}}}\r\n{raw})\r\n",raw.len()).as_bytes()).await.unwrap();
        }
        write(&mut w, b"O1 OK fetched\r\n").await.unwrap();
    });
    let mut p = params(port);
    p["ids"] = json!(["7:INBOX", "8:INBOX"]);
    let result = super::super::call("imap.messages", &p).await.unwrap();
    assert_eq!(result["messages"][0]["id"], "7:INBOX");
    assert_eq!(result["messages"][1]["id"], "8:INBOX");
    assert_eq!(
        result["messages"][0]["labelIds"],
        json!(["UNREAD", "STARRED", "INBOX"])
    );
    assert_eq!(
        result["messages"][0]["payload"]["headers"][1]["value"],
        "Message 7"
    );
    peer.await.unwrap();
}
#[tokio::test]
async fn folder_listing_bounds_uid_arguments_and_preserves_imported_date_order() {
    // Production root cause: a Stalwart server rejects a UID FETCH whose
    // sequence-set argument exceeds 8000 bytes, so a shared Sent Items folder
    // of ~8810 messages failed listing and only a stale cache was shown.
    let uids: Vec<u32> = (1..=8810)
        .chain([3_000_000_000])
        .chain(u32::MAX - 5..=u32::MAX)
        .collect();
    let total = uids.len();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let observed = Arc::new(std::sync::Mutex::new((0usize, Vec::new())));
    let seen = observed.clone();
    let peer = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut w: Wire = BufReader::new(Box::new(stream));
        write(&mut w, b"* OK ready\r\n").await.unwrap();
        assert!(line(&mut w).await.unwrap().starts_with(b"O1 LOGIN"));
        write(&mut w, b"O1 OK login\r\n").await.unwrap();
        for _ in 0..2 {
            assert_eq!(line(&mut w).await.unwrap(), b"O1 CAPABILITY\r\n");
            write(&mut w, b"* CAPABILITY IMAP4rev1\r\nO1 OK caps\r\n")
                .await
                .unwrap();
        }
        assert_eq!(line(&mut w).await.unwrap(), b"O1 LIST \"\" \"*\"\r\n");
        write(&mut w, b"* LIST (\\Sent) \"/\" {10}\r\nSent Items\r\nO1 OK folders\r\n")
            .await
            .unwrap();
        loop {
            let request = String::from_utf8(line(&mut w).await.unwrap()).unwrap();
            let mut response = String::new();
            if request == "O1 SELECT \"Sent Items\"\r\n" {
                response.push_str("O1 OK selected\r\n");
            } else if request == "O1 UID FETCH 1:* (UID)\r\n" {
                for uid in &uids {
                    response.push_str(&format!("* 1 FETCH (UID {uid})\r\n"));
                }
                response.push_str("O1 OK snapshot\r\n");
            } else if let Some(rest) = request.strip_prefix("O1 UID FETCH ") {
                let (set, fields) = rest.split_once(' ').unwrap();
                assert_eq!(fields, "(UID INTERNALDATE)\r\n");
                if set.len() > 8000 {
                    // Stalwart's exact refusal for an over-long argument.
                    write(
                        &mut w,
                        b"O1 BAD [PARSE] Argument exceeds maximum length of 8000 bytes\r\n",
                    )
                    .await
                    .unwrap();
                    continue;
                }
                let batch = expand(set);
                let mut seen = seen.lock().unwrap();
                seen.0 = seen.0.max(set.len());
                seen.1.push(batch.len());
                drop(seen);
                for uid in batch {
                    // Imported reverse date order: the higher the UID, the
                    // older the message, so paging must follow dates, not UIDs.
                    let date = chrono::DateTime::from_timestamp(5_000_000_000 - uid as i64, 0)
                        .unwrap();
                    response.push_str(&format!(
                        "* 1 FETCH (UID {uid} INTERNALDATE \"{}\")\r\n",
                        date.format("%d-%b-%Y %H:%M:%S %z")
                    ));
                }
                response.push_str("O1 OK fetched\r\n");
            } else if let Some(rest) = request.strip_prefix("O1 UID SEARCH UID ") {
                let (range, criteria) = rest.split_once(' ').unwrap();
                assert_eq!(criteria, "UNSEEN\r\n");
                let (first, last) = range.split_once(':').unwrap();
                let (first, last) = (first.parse::<u32>().unwrap(), last.parse::<u32>().unwrap());
                response.push_str("* SEARCH");
                for uid in uids.iter().filter(|uid| **uid >= first && **uid <= last) {
                    response.push_str(&format!(" {uid}"));
                }
                response.push_str("\r\nO1 OK searched\r\n");
            } else {
                panic!("unexpected command: {request}");
            }
            write(&mut w, response.as_bytes()).await.unwrap();
        }
    });
    let mut p = params(port);
    p["query"] = json!("folder:\"Sent Items\"");
    p["limit"] = json!(2);
    let first = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(first["page"]["ids"], json!(["1:Sent Items", "2:Sent Items"]));
    assert_eq!(first["page"]["estimate"], total);
    p["pageToken"] = first["page"]["nextPageToken"].clone();
    let second = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(second["page"]["ids"], json!(["3:Sent Items", "4:Sent Items"]));
    // The last page holds the u32::MAX run, oldest under imported ordering.
    p["pageToken"] = json!((total - 2).to_string());
    let last = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(
        last["page"]["ids"],
        json!(["4294967294:Sent Items", "4294967295:Sent Items"])
    );
    assert_eq!(last["page"]["nextPageToken"], "");
    // A criteria round exercises the bounded SEARCH windows on the same folder.
    p["query"] = json!("folder:\"Sent Items\" UNSEEN");
    p["pageToken"] = json!("");
    let searched = super::super::call("imap.list", &p).await.unwrap();
    assert_eq!(
        searched["page"]["ids"],
        json!(["1:Sent Items", "2:Sent Items"])
    );
    assert_eq!(searched["page"]["estimate"], total);
    let (largest, batches) = &*observed.lock().unwrap();
    assert!(*largest > 0 && *largest <= UID_SET_BYTES);
    assert_eq!(batches.len(), 4 * 3, "every list call re-scans the folder");
    assert!(
        batches.iter().all(|size| *size <= 4096),
        "the response bound stays at 4096 messages per fetch"
    );
    peer.abort();
}
#[tokio::test]
async fn original_query_controls_are_rejected_before_connecting() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    for suffix in ["\r", "\n", "\r\n", "\0", "\t", "\x7f"] {
        for query in ["folder:INBOX UNSEEN", "search:TEXT \"1Password\""] {
            let mut p = params(port);
            p["query"] = json!(format!("{query}{suffix}"));
            assert_eq!(
                super::super::call("imap.list", &p).await,
                Err("invalid_params")
            );
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}
