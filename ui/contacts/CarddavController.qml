import QtQuick
import Quickshell.Io
import "CarddavSources.js" as CarddavSources

// CardDAV address books: the source list is the user's, persisted to
// carddav.json; discovery, refresh and parsing belong to the backend, and the
// password never leaves the keyring RPC. Contacts arrive as snapshots - the
// backend's last complete answer - so a failed refresh keeps suggesting what
// the book held before, and nothing here fetches on a keystroke.
Item {
  id: root

  required property var service

  // The device's own book (Thunderbird/Betterbird profiles, the mail cache,
  // local JSON/vCard files), read by the backend without modifying them.
  // Nothing is copied into Omamail's settings or cache.
  property var localBook: []
  property bool contactsLoading: false

  function refreshLocalContacts() {
    if (contactsLoading || !service || !service.backend || !service.backend.ready) return
    contactsLoading = true
    service.backend.call("contacts.suggest", {}, function(result, error) {
      root.contactsLoading = false
      if (!error && Array.isArray(result)) root.localBook = result
    })
  }

  readonly property bool available: !!service && service.backendCanCarddav === true
  readonly property string configPath: service && typeof service.configPath === "function"
    ? service.configPath("carddav.json") : ""
  property var store: CarddavSources.emptyStore()
  property bool loaded: false
  // The full startup cycle runs once; see beginStartup().
  property bool startupStarted: false
  // sourceId -> { book href -> [ { name, email } ] }; always the backend's last
  // complete answer, never a partially read book.
  property var snapshots: ({})
  // Bumped with every store/snapshot change so account-scoped bindings re-read.
  property int contactsRevision: 0
  property bool busy: false
  property string busySourceId: ""
  property var lastErrors: ({})
  property var pendingQueue: []
  // The startup cycle: one serialized run of every stored snapshot, then
  // every enabled source's refresh. `cycleRunning` refuses a second trigger
  // while answers are still in flight; `cyclePhase` tells the snapshot queue
  // from the refresh queue so a slot released mid-cycle resumes the right
  // one; `cycleGeneration` separates answers of the current run from those
  // of a backend life that already ended.
  property bool cycleRunning: false
  property string cyclePhase: ""
  // Any operation can hold the slot while startup loads snapshots. Resume
  // after its callback finishes, including removal and failed connection cleanup.
  onBusyChanged: if (!busy) Qt.callLater(root.resumeRefreshQueue)
  property int cycleGeneration: 0

  signal connectFinished(bool ok, string error)
  signal refreshFinished(string sourceId, bool ok, string error)
  signal sourceRemoved(string sourceId)

  function describeError(error) {
    return CarddavSources.describeError(error && error.message ? error.message : String(error || ""))
  }

  function contactsForAccount(accountId) {
    var token = root.contactsRevision
    return CarddavSources.contactsForAccount(root.store, root.snapshots, String(accountId || ""))
  }

  function mergedForAccount(accountId) {
    var token = root.contactsRevision
    var local = Array.isArray(root.localBook) ? root.localBook : []
    return CarddavSources.mergeContacts(local, contactsForAccount(accountId))
  }

  function accountLabel(source) {
    var wanted = String(source && source.accountId || "")
    if (wanted === "") return "All accounts"
    var accounts = root.service && Array.isArray(root.service.accountSummaries)
      ? root.service.accountSummaries : []
    for (var i = 0; i < accounts.length; i++) {
      if (String(accounts[i] && accounts[i].id || "") === wanted)
        return String(accounts[i].email || accounts[i].label || wanted)
    }
    return "Mailbox removed"
  }

  function contactCount(source) {
    var count = 0
    var books = root.snapshots[String(source && source.id || "")] || {}
    // `source` may arrive through a Repeater's modelData, where the books
    // list is a QVariantList rather than a JavaScript array — Array.isArray
    // would refuse it and the count would silently stay zero.
    var list = source && source.books && source.books.length !== undefined ? source.books : []
    for (var i = 0; i < list.length; i++) {
      if (list[i] && list[i].enabled === true && books[list[i].href])
        count += books[list[i].href].length
    }
    return count
  }

  function saveStore(next, after, token) {
    var text = CarddavSources.serialize(next)
    if (!root.service || typeof root.service.writeConfig !== "function") {
      if (typeof after === "function") after(false, "Configuration storage is unavailable")
      return
    }
    root.service.writeConfig("carddav.json", text, function(ok, error) {
      // A write already on its way when the backend life ended must not
      // publish the store it was built from.
      var live = token === undefined || token === root.cycleGeneration
      if (ok && live) {
        root.store = next
        root.contactsRevision += 1
      }
      if (typeof after === "function")
        after(ok && live, ok && live ? "" : (live
          ? String(error || "The address book list could not be saved")
          : "The address book list changed while it was saved"))
    })
  }

  // A connect that stored a password but could not finish must not leave it
  // behind unmentioned: delete it, and when even that fails say so, because
  // connecting the same endpoint again is the retry that removes it.
  function finishFailedConnect(sourceId, message) {
    root.service.credentialDelete("carddav-password", sourceId, "", function(removed, cleanupError) {
      var text = message
      if (!removed)
        text += " The stored password could not be removed ("
          + root.describeError(cleanupError) + "); connect again to retry."
      root.busy = false
      root.busySourceId = ""
      root.connectFinished(false, text)
    })
  }

  // Connect = store the password under the URL-derived id, then let the
  // backend discover (or directly verify an explicit collection). A failed
  // connect removes the password it just stored.
  function connect(draft, secret) {
    if (root.busy || root.cycleRunning || !root.available) return false
    var checked = CarddavSources.validateDraft(draft)
    if (!checked.ok) { root.connectFinished(false, checked.error); return true }
    var password = String(secret || "")
    if (password === "" || /[\r\n\u0000]/.test(password)) {
      root.connectFinished(false, "Enter the password or app password")
      return true
    }
    if (CarddavSources.find(root.store, checked.source.id)) {
      root.connectFinished(false, "That address book server is already connected")
      return true
    }
    root.busy = true
    root.busySourceId = checked.source.id
    root.service.credentialPut("carddav-password", checked.source.id, "", password, function(ok, error) {
      password = ""
      if (!ok) {
        root.busy = false
        root.busySourceId = ""
        root.connectFinished(false, root.describeError(error))
        return
      }
      root.discover(checked.source, function(discovered, books) {
        if (!discovered) {
          root.finishFailedConnect(checked.source.id, books)
          return
        }
        var source = checked.source
        source.books = books
        root.saveStore(CarddavSources.add(root.store, source), function(saved, saveError) {
          if (!saved) {
            root.finishFailedConnect(checked.source.id, saveError)
            return
          }
          root.connectFinished(true, "")
          root.refreshContacts(source.id, function(_ok, _error) {
            root.busy = false
            root.busySourceId = ""
          }, root.cycleGeneration)
        })
      })
    })
    return true
  }

  function discover(source, callback) {
    var params = { source: { id: source.id, url: source.url, username: source.username } }
    if (String(source.collection || "") !== "") params.collection = String(source.collection)
    root.service.backend.call("carddav.discover", params, function(result, error) {
      if (error) { callback(false, root.describeError(error)); return }
      var books = []
      var answers = result && Array.isArray(result.books) ? result.books : []
      var seen = {}
      for (var i = 0; i < answers.length; i++) {
        var href = String(answers[i] && answers[i].href || "")
        if (href === "" || seen[href]) continue
        seen[href] = true
        var existing = null
        for (var j = 0; j < (source.books || []).length; j++) {
          if (source.books[j] && source.books[j].href === href) existing = source.books[j]
        }
        books.push({ href: href, name: String(answers[i].name || "Address book"),
          enabled: existing ? existing.enabled !== false : true })
      }
      callback(true, books)
    })
  }

  // One source's contacts: the enabled books, each answer complete or the old
  // snapshot kept. A rediscovery first picks up arrived/departed books.
  // `done` is the startup cycle's continuation; `token` names the backend
  // life this refresh belongs to. An answer from a life that already ended
  // completes the operation — releasing `busy` — without publishing it or
  // advancing a chain.
  function refreshSourceInternal(id, done, token) {
    if (root.busy || !root.available) return false
    var source = CarddavSources.find(root.store, id)
    if (!source) return false
    root.busy = true
    root.busySourceId = source.id
    var finish = function(refreshed, refreshError) {
      root.busy = false
      root.busySourceId = ""
      if (token !== root.cycleGeneration) return
      root.refreshFinished(source.id, refreshed, refreshed ? "" : refreshError)
      if (typeof done === "function") done(refreshed, refreshError)
    }
    root.discover(source, function(ok, books) {
      // The answer arrived after the backend life that asked for it ended:
      // neither write the discovered books nor issue the refresh request.
      if (token !== root.cycleGeneration) { finish(false, ""); return }
      if (!ok) {
        // Discovery is best-effort during a refresh: the stored book list
        // still names what to sync.
        root.refreshContacts(source.id, function(refreshed, refreshError) {
          finish(refreshed, refreshError)
        }, token)
        return
      }
      var next = CarddavSources.add(root.store, {
        id: source.id, url: source.url, username: source.username,
        accountId: source.accountId, enabled: source.enabled,
        collection: source.collection, books: books
      })
      root.saveStore(next, function(saved, saveError) {
        if (!saved) { finish(false, saveError); return }
        root.refreshContacts(source.id, function(refreshed, refreshError) {
          finish(refreshed, refreshError)
        }, token)
      }, token)
    })
    return true
  }

  // A one-off refresh from Settings. While the startup cycle runs, the cycle
  // refreshes every enabled source itself, so this waits for it to end.
  function refreshSource(id) {
    if (root.cycleRunning) return false
    return root.refreshSourceInternal(id, undefined, root.cycleGeneration)
  }

  function refreshContacts(id, callback, token) {
    var source = CarddavSources.find(root.store, id)
    if (!source) { callback(false, ""); return }
    var live = function() { return token === root.cycleGeneration }
    var books = []
    for (var i = 0; i < source.books.length; i++) {
      if (source.books[i] && source.books[i].enabled === true)
        books.push({ href: source.books[i].href })
    }
    if (books.length === 0) {
      if (live()) root.applySnapshot(source.id, ({}))
      callback(true, "")
      return
    }
    root.service.backend.call("carddav.refresh", {
      source: { id: source.id, url: source.url, username: source.username },
      books: books
    }, function(result, error) {
      // The backend life this answer belongs to ended; report completion
      // without publishing.
      if (!live()) { callback(false, ""); return }
      if (error) {
        root.setError(source.id, root.describeError(error))
        callback(false, root.describeError(error))
        return
      }
      var update = {}
      var current = root.snapshots[source.id] || {}
      for (var href in current) update[href] = current[href]
      var answers = result && Array.isArray(result.books) ? result.books : []
      for (var b = 0; b < answers.length; b++) {
        if (answers[b] && answers[b].href && Array.isArray(answers[b].contacts))
          update[String(answers[b].href)] = answers[b].contacts
      }
      var failures = result && Array.isArray(result.errors) ? result.errors : []
      var message = ""
      for (var f = 0; f < failures.length; f++) {
        if (failures[f] && failures[f].error)
          message = root.describeError(failures[f].error)
      }
      // The contacts are in memory either way; say plainly when the cache
      // could not keep them for the next start.
      if (result && result.stored === false)
        message = root.describeError("carddav_snapshot_unavailable")
      root.applySnapshot(source.id, update)
      root.setError(source.id, message)
      callback(true, message)
    })
  }

  function applySnapshot(sourceId, books) {
    var next = {}
    for (var key in root.snapshots) next[key] = root.snapshots[key]
    next[sourceId] = books
    root.snapshots = next
    root.contactsRevision += 1
  }

  function setError(sourceId, message) {
    var next = {}
    for (var key in root.lastErrors) next[key] = root.lastErrors[key]
    if (message === "") delete next[sourceId]
    else next[sourceId] = message
    root.lastErrors = next
  }

  // Removal requires a ready, current CardDAV backend: the source must leave
  // the config, its password must be deleted and its cached snapshot must be
  // forgotten, all three before the row disappears. The config save goes last
  // so a source can never disappear while its password is still stored. A
  // failure at any step keeps the row and its contacts, reports the error on
  // the row, and leaves retry available; a missing password is a successful
  // delete to the credentials API. While the backend cannot forget the cache,
  // removal is refused before any durable step — the snapshot that survived
  // could otherwise come back after a reconnect.
  function removeSource(id) {
    if (root.busy) return false
    var source = CarddavSources.find(root.store, id)
    if (!source) return false
    if (!root.available) {
      root.setError(id, "Update or reconnect the mail backend before removing this address book")
      return false
    }
    root.busy = true
    root.busySourceId = id
    var fail = function(error) {
      root.setError(id, root.describeError(error))
      root.busy = false
      root.busySourceId = ""
    }
    var save = function() {
      root.saveStore(CarddavSources.remove(root.store, id), function(ok, saveError) {
        if (!ok) { fail(saveError); return }
        var cleared = {}
        for (var key in root.snapshots) if (key !== id) cleared[key] = root.snapshots[key]
        root.snapshots = cleared
        root.setError(id, "")
        root.contactsRevision += 1
        root.busy = false
        root.busySourceId = ""
        root.sourceRemoved(id)
      })
    }
    root.service.credentialDelete("carddav-password", id, "", function(ok, error) {
      if (!ok) { fail(error); return }
      root.service.backend.call("carddav.forget", { sourceId: id }, function(_result, forgetError) {
        if (forgetError) {
          fail("The cached contacts could not be removed; try removing this address book again")
          return
        }
        save()
      })
    })
    return true
  }

  function setSourceEnabled(id, enabled) {
    var source = CarddavSources.find(root.store, id)
    if (!source || root.busy) return
    var next = CarddavSources.add(root.store, {
      id: source.id, url: source.url, username: source.username,
      accountId: source.accountId, enabled: enabled === true,
      collection: source.collection, books: source.books
    })
    root.saveStore(next, function(ok, _error) {
      if (ok && enabled === true && root.available) root.refreshSource(id)
    })
  }

  function setSourceAccount(id, accountId) {
    var source = CarddavSources.find(root.store, id)
    if (!source || root.busy) return
    root.saveStore(CarddavSources.add(root.store, {
      id: source.id, url: source.url, username: source.username,
      accountId: String(accountId || ""), enabled: source.enabled,
      collection: source.collection, books: source.books
    }))
  }

  function setBookEnabled(id, href, enabled) {
    var source = CarddavSources.find(root.store, id)
    if (!source || root.busy) return
    var books = []
    for (var i = 0; i < source.books.length; i++) {
      var book = source.books[i]
      books.push({ href: book.href, name: book.name,
        enabled: book.href === href ? enabled === true : book.enabled !== false })
    }
    root.saveStore(CarddavSources.add(root.store, {
      id: source.id, url: source.url, username: source.username,
      accountId: source.accountId, enabled: source.enabled,
      collection: source.collection, books: books
    }), function(ok, _error) {
      if (ok && enabled === true && root.available) root.refreshSource(id)
    })
  }

  // Startup: the local book, every stored snapshot without network, then a
  // refresh of the enabled sources one at a time. Nothing runs on a timer,
  // and one cycle is in flight at a time. `start()` is the single entry,
  // called by the first config answer and by a ready backend, whichever
  // comes last.
  function start() {
    if (!root.service || !root.service.backend || !root.service.backend.ready) return
    if (root.cycleRunning) return
    if (root.loaded) root.refreshLocalContacts()
    if (!root.available || !root.loaded) return
    root.cycleRunning = true
    root.cyclePhase = "snapshots"
    root.cycleGeneration += 1
    var token = root.cycleGeneration
    var sources = root.store.sources
    var ids = []
    for (var i = 0; i < sources.length; i++) if (sources[i]) ids.push(sources[i].id)
    root.pendingQueue = ids
    root.loadNextSnapshot(token)
  }

  function loadNextSnapshot(token) {
    if (token !== root.cycleGeneration) return
    if (root.pendingQueue.length === 0) { root.refreshAll(token); return }
    var id = root.pendingQueue.shift()
    root.service.backend.call("carddav.snapshot", { sourceId: id }, function(result, error) {
      if (token !== root.cycleGeneration) return
      if (!error && result && Array.isArray(result.books)) {
        var books = {}
        for (var i = 0; i < result.books.length; i++) {
          var book = result.books[i]
          if (book && book.href && Array.isArray(book.contacts))
            books[String(book.href)] = book.contacts
        }
        // A source removed while its answer was in flight must not come back
        // through this callback.
        if (CarddavSources.find(root.store, id)) root.applySnapshot(id, books)
      }
      root.loadNextSnapshot(token)
    })
  }

  function refreshAll(token) {
    if (token !== root.cycleGeneration) return
    root.cyclePhase = "refreshes"
    var sources = root.store.sources
    for (var i = 0; i < sources.length; i++) {
      if (sources[i] && sources[i].enabled === true)
        root.pendingQueue.push(sources[i].id)
    }
    root.refreshNext(token)
  }

  function refreshNext(token) {
    if (token !== root.cycleGeneration || !root.cycleRunning) return
    if (root.pendingQueue.length === 0 || !root.available) {
      root.pendingQueue = []
      root.cyclePhase = ""
      root.cycleRunning = false
      return
    }
    // One operation at a time: an older refresh may still be finishing. Its
    // completion hands the slot back through resumeRefreshQueue().
    if (root.busy) return
    var id = root.pendingQueue.shift()
    var started = root.refreshSourceInternal(id, function(_refreshed, _refreshError) {
      root.refreshNext(token)
    }, token)
    if (!started) root.refreshNext(token)
  }

  // A refresh that held the one-at-a-time slot has completed. If the startup
  // cycle is waiting on that slot, continue its refresh queue.
  function resumeRefreshQueue() {
    if (root.cycleRunning && root.cyclePhase === "refreshes" && root.pendingQueue.length > 0)
      root.refreshNext(root.cycleGeneration)
  }

  // The full startup cycle runs once: the first config answer and a ready
  // backend, whichever comes last. A later config reload — including the
  // ones this controller's own saves cause — only updates the store; the
  // action that saved did its targeted work already.
  function beginStartup() {
    if (root.startupStarted) return
    root.startupStarted = true
    root.start()
  }

  FileView {
    path: root.configPath
    watchChanges: true
    printErrors: false
    // Only the initial load starts the pipeline. Later reloads — external
    // edits, or the write this controller itself just made — update the
    // store and republish through contactsRevision without another full
    // snapshot/refresh cycle on top of the targeted refresh that caused
    // them.
    onLoaded: {
      root.store = CarddavSources.load(text())
      root.loaded = true
      root.contactsRevision += 1
      root.beginStartup()
    }
    onFileChanged: reload()
    // A first start with no config file is a normal start: an empty store
    // still loads the local book and runs the pipeline.
    onLoadFailed: {
      if (!root.store || !Array.isArray(root.store.sources) || root.store.sources.length > 0) {
        root.store = CarddavSources.emptyStore()
        root.contactsRevision += 1
      }
      root.loaded = true
      root.beginStartup()
    }
  }

  Connections {
    target: root.service ? root.service.backend : null
    function onReadyChanged() {
      var ready = root.service && root.service.backend && root.service.backend.ready
      if (ready) { root.start(); return }
      // The backend went away. Answers still in flight belong to the life
      // that ended: retire the cycle so a reconnect starts one fresh cycle,
      // and leave `busy` to the operation that owns it — its own callback
      // releases it, including when the answer turns out to be stale.
      root.cycleGeneration += 1
      root.cycleRunning = false
      root.cyclePhase = ""
      root.pendingQueue = []
      root.contactsLoading = false
    }
  }
}
