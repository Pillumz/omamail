const assert = require("assert")
const { load } = require("./load")
const carddav = load("contacts/CarddavSources.js")

// The loader runs modules in a VM realm; compare JSON values, not prototypes.
function j(value) { return JSON.parse(JSON.stringify(value)) }

// SHA-256 reference vectors, and the backend known-answer vector: the id is
// computed identically here and in `carddav::derive_source_id`, or the
// keyring binding refuses the connect.
assert.strictEqual(carddav.sha256hex(""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
assert.strictEqual(carddav.sha256hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
assert.strictEqual(carddav.sha256hex("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
  "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1")
assert.strictEqual(carddav.sha256hex("Петров"), "d9a7cdac33d1f47941ee0f52a0083aaa4ef9c2cbab782a77899629051f85ccb6",
  "the key is hashed as UTF-8")

const staffId = "carddav:9ff5d0d63e48ae4dd33ec08f3adbaa748bd131b7f320641e9438adf4c7e8f119"
assert.strictEqual(carddav.deriveId("https://dav.example.org/dav/card/", "employee@example.org"), staffId)

// Spellings of one endpoint share an identity: scheme and host case, the
// default port, an omitted path, surrounding whitespace.
for (const spelling of ["https://dav.example.org:443/dav/card/",
    "HTTPS://Dav.Example.ORG/dav/card/", "  https://dav.example.org/dav/card/  "]) {
  assert.strictEqual(carddav.deriveId(spelling, "employee@example.org"), staffId, spelling)
}
// Spellings that slug-based ids would merge but that name different
// endpoints — or the same endpoint under another account — must not.
const distinct = [
  carddav.deriveId("https://a.b/", "ann"),
  carddav.deriveId("https://a-b/", "ann"),
  carddav.deriveId("https://dav.example.org/dav/card/", "anna"),
  carddav.deriveId("https://dav.example.org:4443/dav/card/", "employee@example.org"),
  carddav.deriveId("https://dav.example.org/dav/other/", "employee@example.org")
]
for (let a = 0; a < distinct.length; a++) {
  assert.ok(/^carddav:[0-9a-f]{64}$/.test(distinct[a]), distinct[a])
  for (let b = a + 1; b < distinct.length; b++) assert.notStrictEqual(distinct[a], distinct[b])
}

// The canonical form rejects anything the backend would.
for (const bad of ["http://dav.example.org/", "dav.example.org", "https://dav.example.org/#x",
    "https://dav.example.org/?q=1", "https://user@dav.example.org/", "https://dav.example.org/a b",
    "https://dav.example.org/a\\b", "https:///path", "https://dav.example.org:99999/",
    "https://dav.example.org:", "https://дав.example.org/"]) {
  assert.strictEqual(carddav.canonicalParts(bad), null, bad)
}

// Origins agree across default-port spellings: an explicit collection on
// https://host:443 is the same server as the source on https://host.
assert.strictEqual(carddav.originOf("https://dav.example.org:443/dav/card/"), "https://dav.example.org")
assert.strictEqual(carddav.originOf("https://dav.example.org/dav/card/"), "https://dav.example.org")
assert.strictEqual(carddav.originOf("https://dav.example.org:4443/"), "https://dav.example.org:4443")

// The draft gate: https shape, a username, and an explicit collection that
// stays on the source's origin.
let checked = carddav.validateDraft({ url: "https://dav.example.org/dav/card/",
  username: "employee@example.org" })
assert.ok(checked.ok)
assert.strictEqual(checked.source.id, staffId)
assert.strictEqual(checked.source.collection, "")
assert.ok(!carddav.validateDraft({ url: "http://dav.example.org/", username: "u" }).ok)
assert.ok(!carddav.validateDraft({ url: "https://dav.example.org/", username: "" }).ok)
assert.ok(!carddav.validateDraft({ url: "https://dav.example.org/", username: "bad name" }).ok)
checked = carddav.validateDraft({ url: "https://dav.example.org/dav/card/",
  username: "u", collection: "https://dav.example.org:443/dav/card/directory%40example.org/staff/" })
assert.ok(checked.ok, "an explicit collection on the same origin, default port spelled out, is allowed")
assert.ok(!carddav.validateDraft({ url: "https://dav.example.org/", username: "u",
  collection: "https://dav.example.org:4443/card/" }).ok, "another port is another origin")
assert.ok(!carddav.validateDraft({ url: "https://dav.example.org/", username: "u",
  collection: "https://other.example.org/card/" }).ok, "a collection on another origin is refused")

// Store round trip: sources normalize, replace by id and survive JSON.
let store = carddav.emptyStore()
store = carddav.add(store, checked.source)
store = carddav.add(store, carddav.validateDraft({ url: "https://fastmail.example.com/",
  username: "me", accountId: "imap:me@example.com" }).source)
assert.strictEqual(store.sources.length, 2)
store = carddav.add(store, { id: checked.source.id,
  url: "https://dav.example.org/dav/card/", username: "u",
  books: [{ href: "https://dav.example.org/book/", name: "CDM Staff", enabled: true }] })
assert.strictEqual(store.sources.length, 2, "a same-id source replaces")
const loaded = carddav.load(carddav.serialize(store))
assert.strictEqual(loaded.sources.length, 2)
assert.strictEqual(loaded.sources[0].books[0].name, "CDM Staff")
assert.strictEqual(carddav.load("not json").sources.length, 0)
assert.strictEqual(carddav.load('{"sources":[{"id":"","url":""}]}').sources.length, 0)

// Account scoping: a linked source never leaks into another mailbox; an
// unlinked one serves every account; nothing serves an empty one.
store = carddav.emptyStore()
store = carddav.add(store, { id: "carddav:" + "1".repeat(64), url: "https://work.example/",
  username: "u", accountId: "imap:me@work.example",
  books: [{ href: "https://work.example/staff/", name: "Staff", enabled: true },
          { href: "https://work.example/ex/", name: "Former", enabled: false }] })
store = carddav.add(store, { id: "carddav:" + "2".repeat(64), url: "https://shared.example/",
  username: "u", accountId: "",
  books: [{ href: "https://shared.example/all/", name: "All", enabled: true }] })
const workId = "carddav:" + "1".repeat(64)
const sharedId = "carddav:" + "2".repeat(64)
const snapshots = {}
snapshots[workId] = {
  "https://work.example/staff/": [{ name: "Иван Петров", email: "ivan@work.example" }],
  "https://work.example/ex/": [{ name: "Gone", email: "gone@work.example" }]
}
snapshots[sharedId] = {
  "https://shared.example/all/": [{ name: "Shared", email: "shared@example.org" }]
}
let contacts = carddav.contactsForAccount(store, snapshots, "imap:me@work.example")
assert.deepStrictEqual(j(contacts.map(function(c) { return c.email }).sort()),
  ["ivan@work.example", "shared@example.org"])
contacts = carddav.contactsForAccount(store, snapshots, "gmail:me@personal.example")
assert.deepStrictEqual(j(contacts.map(function(c) { return c.email })), ["shared@example.org"],
  "the corporate book stays out of an unrelated account")
assert.deepStrictEqual(j(carddav.contactsForAccount(store, snapshots, "")), [{ name: "Shared", email: "shared@example.org" }])
// A disabled source or book contributes nothing.
store = carddav.add(store, { id: workId, url: "https://work.example/",
  username: "u", accountId: "imap:me@work.example", enabled: false,
  books: [{ href: "https://work.example/staff/", name: "Staff", enabled: true }] })
contacts = carddav.contactsForAccount(store, snapshots, "imap:me@work.example")
assert.deepStrictEqual(j(contacts.map(function(c) { return c.email })), ["shared@example.org"])

// Merging: local entries keep their place; a remote name completes a bare
// local address, and duplicates collapse case-insensitively.
const merged = carddav.mergeContacts(
  [{ name: "", email: "ivan@work.example" }, { name: "Local", email: "local@example.org" }],
  [{ name: "Иван Петров", email: "Ivan@work.example" }, { name: "Мария", email: "maria@work.example" }])
assert.strictEqual(merged.length, 3)
const ivan = merged.filter(function(c) { return c.email === "ivan@work.example" })[0]
assert.strictEqual(ivan.name, "Иван Петров")
assert.strictEqual(merged[0].name, "Local", "sorted by name, local names kept")

assert.ok(carddav.describeError("carddav_auth_refused").length > 0)
assert.ok(carddav.describeError("carddav_snapshot_unavailable").indexOf("until the app restarts") !== -1)
assert.ok(carddav.describeError("carddav_total_too_large").indexOf("disable some of them") !== -1,
  "an aggregate overflow is said as something the user can act on")
assert.ok(carddav.describeError("unlisted-code") === "unlisted-code")

console.log("test_carddav_sources ok")
