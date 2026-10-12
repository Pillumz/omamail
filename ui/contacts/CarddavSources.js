.pragma library

// CardDAV source records and their contact projection. Presentation rules
// only: the backend owns discovery, refresh, parsing and the credential.
// A source record is { id, url, username, accountId, enabled, collection,
// books: [{ href, name, enabled }] } and carries no secret.

var VERSION = 1

function emptyStore() { return { version: VERSION, sources: [] } }

function trimmed(value) {
  return String(value === undefined || value === null ? "" : value).trim()
}

function hasControl(value) {
  for (var i = 0; i < value.length; i++) {
    var code = value.charCodeAt(i)
    if (code < 32 || code === 127) return true
  }
  return false
}

// SHA-256, pure ECMAScript. The source id is a hash of the canonical
// endpoint and username, computed identically here and in the backend.
function sha256hex(text) {
  var K = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2
  ]
  var encoded = unescape(encodeURIComponent(text))
  var bytes = []
  var i
  for (i = 0; i < encoded.length; i++) bytes.push(encoded.charCodeAt(i))
  var bitLength = bytes.length * 8
  bytes.push(0x80)
  while (bytes.length % 64 !== 56) bytes.push(0)
  bytes.push(Math.floor(bitLength / 0x100000000) & 255, 0, 0, 0)
  bytes.push((bitLength >>> 24) & 255, (bitLength >>> 16) & 255, (bitLength >>> 8) & 255, bitLength & 255)
  var h = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]
  var w = new Array(64)
  var rotr = function(x, n) { return ((x >>> n) | (x << (32 - n))) | 0 }
  for (var block = 0; block < bytes.length; block += 64) {
    for (i = 0; i < 16; i++)
      w[i] = ((bytes[block + i * 4] << 24) | (bytes[block + i * 4 + 1] << 16)
        | (bytes[block + i * 4 + 2] << 8) | bytes[block + i * 4 + 3]) | 0
    for (i = 16; i < 64; i++) {
      var s0 = (rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >>> 3)) | 0
      var s1 = (rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >>> 10)) | 0
      w[i] = (w[i - 16] + s0 + w[i - 7] + s1) | 0
    }
    var a = h[0], b = h[1], c = h[2], d = h[3], e = h[4], f = h[5], g = h[6], hh = h[7]
    for (i = 0; i < 64; i++) {
      var sum1 = (rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)) | 0
      var choose = ((e & f) ^ (~e & g)) | 0
      var t1 = (hh + sum1 + choose + K[i] + w[i]) | 0
      var sum0 = (rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)) | 0
      var majority = ((a & b) ^ (a & c) ^ (b & c)) | 0
      var t2 = (sum0 + majority) | 0
      hh = g; g = f; f = e; e = (d + t1) | 0; d = c; c = b; b = a; a = (t1 + t2) | 0
    }
    h[0] = (h[0] + a) | 0; h[1] = (h[1] + b) | 0; h[2] = (h[2] + c) | 0; h[3] = (h[3] + d) | 0
    h[4] = (h[4] + e) | 0; h[5] = (h[5] + f) | 0; h[6] = (h[6] + g) | 0; h[7] = (h[7] + hh) | 0
  }
  var out = ""
  for (i = 0; i < 8; i++) out += ("0000000" + (h[i] >>> 0).toString(16)).slice(-8)
  return out
}

// The canonical form of a source URL: https, printable ASCII only, scheme
// and host lowercased, the default port elided, an empty path shown as "/".
// Everything else is kept verbatim, so two spellings share an identity only
// when they name the same endpoint. Mirrors `carddav::canonical_parts`.
function canonicalParts(url) {
  var text = trimmed(url)
  if (text.length < 9 || text.length > 2048) return null
  if (text.indexOf("\\") !== -1) return null
  for (var i = 0; i < text.length; i++) {
    var code = text.charCodeAt(i)
    if (code < 0x21 || code > 0x7e) return null
  }
  if (text.substring(0, 8).toLowerCase() !== "https://") return null
  var rest = text.substring(8)
  var pathStart = rest.length
  for (i = 0; i < rest.length; i++) {
    var ch = rest.charAt(i)
    if (ch === "/" || ch === "?" || ch === "#") { pathStart = i; break }
  }
  var authority = rest.substring(0, pathStart)
  var tail = rest.substring(pathStart)
  if (authority === "" || authority.indexOf("@") !== -1) return null
  if (tail.indexOf("?") !== -1 || tail.indexOf("#") !== -1) return null
  var host = ""
  var port = null
  if (authority.charAt(0) === "[") {
    var close = authority.indexOf("]")
    if (close === -1) return null
    host = authority.substring(0, close + 1)
    if (authority.length > close + 1) {
      if (authority.charAt(close + 1) !== ":") return null
      port = authority.substring(close + 2)
    }
  } else {
    var colon = authority.lastIndexOf(":")
    if (colon !== -1) { host = authority.substring(0, colon); port = authority.substring(colon + 1) }
    else host = authority
  }
  host = host.toLowerCase()
  if (host === "" || host.indexOf(":") !== -1) return null
  var portPart = ""
  if (port !== null) {
    if (!/^[0-9]{1,5}$/.test(port)) return null
    var number = parseInt(port, 10)
    if (number > 65535) return null
    if (number !== 443) portPart = ":" + number
  }
  var origin = "https://" + host + portPart
  return { origin: origin, canonical: origin + (tail === "" ? "/" : tail) }
}

// The stable identity of one address book source: the SHA-256 of the
// canonical endpoint and the username. Two spellings of one endpoint share
// an id and therefore one keyring entry; two endpoints or two accounts on
// one server never do. Mirrors `carddav::derive_source_id` exactly.
function deriveId(url, username) {
  var parts = canonicalParts(url)
  if (!parts) return ""
  return "carddav:" + sha256hex(parts.canonical + "\n" + trimmed(username))
}

function makeBook(raw) {
  var value = raw || {}
  return { href: trimmed(value.href), name: trimmed(value.name), enabled: value.enabled !== false }
}

function makeSource(raw) {
  var value = raw || {}
  var books = []
  var seen = {}
  var rawBooks = Array.isArray(value.books) ? value.books : []
  for (var i = 0; i < rawBooks.length; i++) {
    var book = makeBook(rawBooks[i])
    if (book.href === "" || seen[book.href]) continue
    seen[book.href] = true
    books.push(book)
  }
  return {
    id: trimmed(value.id),
    url: trimmed(value.url),
    username: trimmed(value.username),
    accountId: trimmed(value.accountId),
    enabled: value.enabled !== false,
    collection: trimmed(value.collection),
    books: books
  }
}

function load(text) {
  var parsed
  try { parsed = JSON.parse(String(text || "")) } catch (e) { return emptyStore() }
  if (!parsed || !Array.isArray(parsed.sources)) return emptyStore()
  var store = emptyStore()
  for (var i = 0; i < parsed.sources.length; i++) {
    var source = makeSource(parsed.sources[i])
    if (source.id === "" || source.url === "" || source.username === "") continue
    store = add(store, source)
  }
  return store
}

function serialize(store) {
  var value = store || emptyStore()
  return JSON.stringify({ version: VERSION, sources: value.sources })
}

function add(store, raw) {
  var value = store || emptyStore()
  var source = makeSource(raw)
  var next = { version: VERSION, sources: value.sources.slice() }
  if (source.id === "") return next
  for (var i = 0; i < next.sources.length; i++) {
    if (next.sources[i] && next.sources[i].id === source.id) {
      next.sources[i] = source
      return next
    }
  }
  next.sources.push(source)
  return next
}

function remove(store, id) {
  var value = store || emptyStore()
  var wanted = trimmed(id)
  return { version: VERSION, sources: value.sources.filter(function(source) {
    return source && source.id !== wanted
  }) }
}

function find(store, id) {
  var value = store || emptyStore()
  var wanted = trimmed(id)
  for (var i = 0; i < value.sources.length; i++) {
    if (value.sources[i] && value.sources[i].id === wanted) return value.sources[i]
  }
  return null
}

// The connect form's own gate; the backend re-checks everything before the
// credential is touched.
function validateDraft(draft) {
  var value = draft || {}
  var url = trimmed(value.url)
  var parts = canonicalParts(url)
  if (!parts) return { ok: false, error: "Enter the server URL as https:// followed by the server name, without spaces, query or #" }
  var username = trimmed(value.username)
  if (username === "" || hasControl(username) || /\s/.test(username))
    return { ok: false, error: "Enter the username" }
  var collection = trimmed(value.collection)
  if (collection !== "") {
    var bookParts = canonicalParts(collection)
    if (!bookParts)
      return { ok: false, error: "Enter the address book URL as https:// or leave it empty" }
    if (bookParts.origin !== parts.origin)
      return { ok: false, error: "The address book URL must stay on the same server" }
  }
  return { ok: true, source: {
    id: deriveId(url, username), url: url, username: username,
    accountId: trimmed(value.accountId), enabled: true, collection: collection,
    books: []
  } }
}

function originOf(url) {
  var parts = canonicalParts(url)
  return parts ? parts.origin : ""
}

// What a row names its source: the server, then the path when it says
// something the server name alone does not.
function sourceLabel(source) {
  var value = source || {}
  var parts = canonicalParts(value.url)
  if (!parts) return trimmed(value.url) || "CardDAV"
  var path = parts.canonical.substring(parts.origin.length).replace(/\/+$/, "")
  if (path === "" || path === "/dav" || path === "/dav/card") return parts.origin
  return parts.origin + path
}

// The remote contacts one sender account may see: enabled sources linked to
// that account, plus unlinked ones. A linked source never leaks into an
// unrelated mailbox, and an orphan (its mailbox was removed) shows nowhere.
function contactsForAccount(store, snapshots, accountId) {
  var wanted = trimmed(accountId)
  var out = []
  var value = store || emptyStore()
  var booksBySource = snapshots || {}
  for (var i = 0; i < value.sources.length; i++) {
    var source = value.sources[i]
    if (!source || source.enabled !== true) continue
    if (trimmed(source.accountId) !== "" && trimmed(source.accountId) !== wanted) continue
    if (wanted === "" && trimmed(source.accountId) !== "") continue
    var books = booksBySource[source.id] || {}
    for (var j = 0; j < source.books.length; j++) {
      var book = source.books[j]
      if (!book || book.enabled !== true) continue
      var contacts = books[book.href] || []
      for (var k = 0; k < contacts.length; k++) out.push(contacts[k])
    }
  }
  return out
}

// Local entries stay first-class; a remote row only adds an address the
// device does not know, or a name for one it does.
function mergeContacts(local, remote) {
  var out = []
  var byEmail = {}
  var i
  var values = (Array.isArray(local) ? local : []).concat(Array.isArray(remote) ? remote : [])
  for (i = 0; i < values.length; i++) {
    var contact = values[i] || {}
    var email = trimmed(contact.email)
    var at = email.indexOf("@")
    if (at <= 0 || at === email.length - 1) continue
    var key = email.toLowerCase()
    var name = trimmed(contact.name)
    if (byEmail[key] === undefined) {
      byEmail[key] = out.length
      out.push({ name: name, email: email })
    } else if (out[byEmail[key]].name === "" && name !== "") {
      out[byEmail[key]].name = name
    }
  }
  out.sort(function(left, right) {
    var a = (left.name || left.email).toLowerCase()
    var b = (right.name || right.email).toLowerCase()
    return a < b ? -1 : (a > b ? 1 : 0)
  })
  return out
}

// Static backend refusal names, said the way a settings page can act on them.
function describeError(code) {
  var text = trimmed(code)
  if (text === "carddav_invalid_url") return "The server URL must be a plain https:// address"
  if (text === "carddav_origin_refused") return "The server pointed at a different server; the address book must stay on the same origin"
  if (text === "carddav_password_missing") return "No password is stored for this address book; connect it again"
  if (text === "carddav_password_invalid") return "The stored password cannot be used; connect it again"
  if (text === "carddav_keyring_failed" || text === "credential_store_unavailable") return "The system keyring is unavailable"
  if (text === "carddav_auth_refused") return "The server refused the username or password"
  if (text === "carddav_not_found") return "The server has nothing at that address"
  if (text === "carddav_not_addressbook") return "That address is not an address book"
  if (text === "carddav_discovery_failed") return "The server did not name an address book home; enter the address book URL directly"
  if (text === "carddav_too_many_books") return "The server listed more address books than supported"
  if (text === "carddav_too_many_contacts") return "An address book is larger than supported"
  if (text === "carddav_total_too_large") return "The enabled address books are together larger than supported; disable some of them and refresh again"
  if (text === "carddav_response_too_large") return "The server's answer is larger than supported"
  if (text === "carddav_timeout" || text === "carddav_network_failed") return "The server could not be reached"
  if (text === "carddav_invalid_response") return "The server's answer could not be read"
  if (text === "carddav_snapshot_unavailable") return "The refreshed contacts could not be stored locally; they stay available until the app restarts"
  if (text === "backend_needs_update") return "Update the mail backend to connect CardDAV address books"
  return text === "" ? "The address book could not be reached" : text
}
