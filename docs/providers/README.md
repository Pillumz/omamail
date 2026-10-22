# Provider invariants

Required when changing provider adapters, account identity, capabilities or provider-dependent UI.

- Keep provider differences in descriptors/adapters and the registry, not higher-level views. Chooser order is Gmail, Outlook, Microsoft 365, HEY, JMAP, IMAP; prefer the specific protocol before the catch-all.
- Auth and fetching adapters expose the same methods and callback shapes to `MailAccount`.
- Every client returns Gmail's message-resource shape: headers, MIME tree and base64url bodies. RFC 822 adapters use `Message.parseRfc822`; HEY constructs the same shape from postings and topics. Conversation listings add the `thread` block consumed by `Message.threadOf`.
- Detail reads update only fields they carry (`Model.detailSummary`); absence must not erase listing metadata.
- Provider capabilities are a ceiling. Account refusals can remove capabilities, never add them. Hide unsupported buttons and mailbox roles, filter key hints, and refuse actions in `MailAccount.act` before optimistic updates. Hiding a button does not disable its shortcut.
- `threads` means server thread identity; `conversations` means a collapsed listing. Do not infer one from the other.
- Gmail account IDs remain the lower-cased address for compatibility. Other IDs are `<provider>:<lower-cased-address>`; the same address can identify different providers.
- Resolve web locations through `Registry.webMessageUrl` / `webBoxUrl`.
- `Registry.mark` supplies the square icon and `logo` the lockup; preserve official artwork colors. Unbranded providers use the themed envelope.

Read the provider-specific [HEY](hey.md), [IMAP](imap.md), [JMAP](jmap.md) or [Microsoft 365](m365.md) notes when applicable, and [security boundaries](../SECURITY-BOUNDARIES.md) before touching transport or authentication.

## CardDAV contact sources

CardDAV address books are read-only contact sources beside the local book: Rust owns discovery, refresh, parsing, credentials and snapshots (`src/carddav/`, API 9's `carddav.*` methods, gated in QML by `Service.backendCanCarddav`). Every answer is validated strictly — a 207 Multi-Status, one closed `{DAV:}` multistatus root with namespace-resolved elements, and propstat statuses that say the properties came back; a truncated, foreign or erroring answer is refused whole so it can never replace the last complete snapshot. The addressbook-query carries the RFC 6352 mandatory filter (an empty `<c:filter/>`, the all-contacts query). Requests stay on the source's verified-HTTPS origin with its own Basic credential; redirects are followed by hand, same-origin only.

`src/carddav/tls_test.py` is the maintained synthetic peer for all of it: a loopback HTTPS server with a disposable certificate (SANs `dav.example.org`, `localhost`, `carddav-peer.invalid`, `127.0.0.1`) driven by JSON lines on stdin. Scripted mode answers a queue (the Rust runtime tests); `{"serve": true}` switches it to a small directory — `https://127.0.0.1:<port>/dav/` with username `synthetic@example.org` and password `synthetic` — serving two books (`Personal contacts`, `Справочник компании`) with Cyrillic contacts, requiring Basic auth on every request and refusing a REPORT without the filter, like a strict server. `{"faults": {"/dav/directory/": 503}}` arms a one-shot failure; `{"responses": [...]}` scripts raw answers. To drive the real UI against it, build the backend with `--features integration-test-credentials` (debug only; release builds refuse the feature), run it with `SSL_CERT_FILE` pointing at the reported certificate and `OMAMAIL_INTEGRATION_CREDENTIAL_FILE` at a file containing `synthetic\n`, and point the guest at the reported port. `tests/test_backend_api.py --carddav-peer` wires exactly this for the contract.
