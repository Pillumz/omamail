import QtQuick 2.15
import QtTest 1.3
import "../../contacts" as Omamail
import "../../contacts/CarddavSources.js" as CarddavSources

Item {
  width: 400
  height: 300

  QtObject {
    id: mailService

    property bool backendCanCarddav: true
    property var accountSummaries: [
      { id: "imap:work@example.com", email: "work@example.com" },
      { id: "imap:personal@example.org", email: "personal@example.org" }
    ]
    property var credentialCalls: []
    property var backendCalls: []
    property var configWrites: []
    property var discoverResult: null
    property var discoverError: null
    property var refreshResult: null
    property var refreshError: null
    property var snapshotResult: null
    property var localResult: []
    property var backend: ({ ready: true, call: function(method, params, callback) {
      mailService.backendCalls.push({ method: method, params: params })
      if (method === "contacts.suggest") { callback(mailService.localResult, null); return }
      if (method === "carddav.discover") { callback(mailService.discoverResult, mailService.discoverError); return }
      if (method === "carddav.refresh") { callback(mailService.refreshResult, mailService.refreshError); return }
      if (method === "carddav.snapshot") { callback(mailService.snapshotResult || { books: [] }, null); return }
      if (method === "carddav.forget") { callback({ forgotten: true }, null); return }
      callback(null, { code: -32601, message: "unknown_method" })
    } })

    function configPath(name) { return "/tmp/omamail-test/" + name }
    function writeConfig(name, payload, callback) {
      configWrites.push({ name: name, payload: payload })
      callback(true, "")
    }
    function credentialPut(kind, accountId, clientId, secret, callback) {
      credentialCalls.push({ op: "put", kind: kind, id: accountId, secret: secret })
      callback(true, "")
      return true
    }
    function credentialDelete(kind, accountId, clientId, callback) {
      credentialCalls.push({ op: "delete", kind: kind, id: accountId })
      callback(true, "")
      return true
    }
  }

  Omamail.CarddavController {
    id: controller
    service: mailService
    loaded: true
  }

  TestCase {
    name: "CarddavController"

    // The Rust known-answer vector for (https://dav.example.org/dav/card/,
    // employee@example.org): the UI must derive the id the backend derives.
    property string staffId: "carddav:9ff5d0d63e48ae4dd33ec08f3adbaa748bd131b7f320641e9438adf4c7e8f119"
    property var draft: ({ url: "https://dav.example.org/dav/card/",
      username: "employee@example.org", accountId: "imap:work@example.com" })

    function init() {
      mailService.backendCanCarddav = true
      mailService.credentialCalls = []
      mailService.backendCalls = []
      mailService.configWrites = []
      mailService.discoverResult = null
      mailService.discoverError = null
      mailService.refreshResult = null
      mailService.refreshError = null
      mailService.snapshotResult = null
      mailService.localResult = []
      controller.store = CarddavSources.emptyStore()
      controller.snapshots = ({})
      controller.lastErrors = ({})
      controller.pendingQueue = []
      controller.busy = false
      controller.busySourceId = ""
      controller.localBook = []
      controller.loaded = true
      // The controller's own FileView answer (absent or leftover test file)
      // must not start anything during a test: each test drives startup itself.
      controller.startupStarted = true
    }

    function connectWork() {
      mailService.discoverResult = { books: [
        { href: "https://dav.example.org/dav/card/directory%40example.org/staff/", name: "CDM Staff" }
      ] }
      mailService.refreshResult = { books: [
        { href: "https://dav.example.org/dav/card/directory%40example.org/staff/",
          contacts: [{ name: "Иван Петров", email: "ivan@example.org" }] }
      ], errors: [], stored: true }
      verify(controller.connect(draft, "secret-1"))
      compare(controller.busy, false)
    }

    function test_connect_stores_the_password_under_the_derived_id_and_discovers() {
      connectWork()
      compare(mailService.credentialCalls.length, 1)
      compare(mailService.credentialCalls[0].op, "put")
      compare(mailService.credentialCalls[0].kind, "carddav-password")
      compare(mailService.credentialCalls[0].id, staffId,
        "the UI derives the id the backend derives")
      var calls = mailService.backendCalls.map(function(call) { return call.method })
      compare(JSON.stringify(calls), JSON.stringify(["carddav.discover", "carddav.refresh"]))
      compare(mailService.backendCalls[0].params.source.id, staffId)
      verify(JSON.stringify(mailService.backendCalls).indexOf("secret-1") === -1,
        "the password never travels in a backend call")
      compare(mailService.configWrites.length, 1)
      compare(mailService.configWrites[0].name, "carddav.json")
      var saved = JSON.parse(mailService.configWrites[0].payload).sources[0]
      compare(saved.id, staffId)
      compare(saved.books[0].name, "CDM Staff")
      // The connected book publishes into the linked account's book only.
      var work = controller.mergedForAccount("imap:work@example.com")
      compare(work.length, 1)
      compare(work[0].name, "Иван Петров")
      compare(controller.mergedForAccount("imap:personal@example.org").length, 0,
        "the corporate directory stays out of an unrelated mailbox")
    }

    function test_a_failed_connect_removes_only_its_own_password() {
      connectWork()
      mailService.credentialCalls = []
      mailService.discoverResult = null
      mailService.discoverError = { code: -32000, message: "carddav_auth_refused" }
      var failed = ""
      var ok = false
      var spy = function(good, error) { ok = good; failed = error }
      controller.connectFinished.connect(spy)
      verify(controller.connect({ url: "https://other.example.org/card/",
        username: "bob", accountId: "" }, "secret-2"))
      controller.connectFinished.disconnect(spy)
      compare(ok, false)
      verify(failed.length > 0)
      compare(JSON.stringify(mailService.credentialCalls.map(function(call) { return call.op + ":" + call.id })),
        JSON.stringify(["put:" + CarddavSources.deriveId("https://other.example.org/card/", "bob"),
                        "delete:" + CarddavSources.deriveId("https://other.example.org/card/", "bob")]),
        "a failed connect removes the password it stored, and only that one")
      compare(controller.store.sources.length, 1, "the working source stays connected")
    }

    function test_reconnecting_the_same_endpoint_is_refused_before_the_keyring() {
      connectWork()
      mailService.credentialCalls = []
      verify(controller.connect(draft, "secret-2"))
      compare(controller.busy, false)
      compare(mailService.credentialCalls.length, 0,
        "the same endpoint and username is already connected; nothing is overwritten")
    }

    function test_disable_remove_and_account_change_republish_the_book() {
      connectWork()
      controller.setSourceEnabled(staffId, false)
      compare(controller.mergedForAccount("imap:work@example.com").length, 0,
        "a disabled source suggests nothing")
      controller.setSourceEnabled(staffId, true)
      compare(controller.mergedForAccount("imap:work@example.com").length, 1)
      controller.setSourceAccount(staffId, "imap:personal@example.org")
      compare(controller.mergedForAccount("imap:work@example.com").length, 0)
      compare(controller.mergedForAccount("imap:personal@example.org").length, 1,
        "linking follows the source, not the contacts")
      mailService.credentialCalls = []
      mailService.backendCalls = []
      verify(controller.removeSource(staffId))
      compare(mailService.credentialCalls.length, 1)
      compare(mailService.credentialCalls[0].op, "delete")
      compare(mailService.credentialCalls[0].id, staffId)
      compare(mailService.backendCalls.map(function(call) { return call.method }), ["carddav.forget"])
      compare(controller.store.sources.length, 0)
      compare(controller.mergedForAccount("imap:personal@example.org").length, 0)
    }

    function test_a_failed_refresh_keeps_the_previous_contacts() {
      mailService.snapshotResult = { books: [
        { href: "https://dav.example.org/dav/card/directory%40example.org/staff/",
          contacts: [{ name: "Earlier", email: "earlier@example.org" }] }
      ] }
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      mailService.refreshError = { code: -32000, message: "carddav_auth_refused" }
      controller.store = CarddavSources.add(controller.store, {
        id: staffId, url: "https://dav.example.org/dav/card/", username: "employee@example.org",
        accountId: "imap:work@example.com", enabled: true,
        books: [{ href: "https://dav.example.org/dav/card/directory%40example.org/staff/",
          name: "CDM Staff", enabled: true }]
      })
      controller.start()
      var work = controller.mergedForAccount("imap:work@example.com")
      compare(work.length, 1)
      compare(work[0].email, "earlier@example.org",
        "a refused refresh leaves the last complete answer in place")
      verify(String(controller.lastErrors[staffId] || "").length > 0)
    }

    function test_an_unpersisted_refresh_is_said_out_loud() {
      connectWork()
      mailService.refreshResult = { books: [
        { href: "https://dav.example.org/dav/card/directory%40example.org/staff/",
          contacts: [{ name: "Иван Петров", email: "ivan@example.org" }] }
      ], errors: [], stored: false }
      verify(controller.refreshSource(staffId))
      compare(controller.busy, false)
      var warning = String(controller.lastErrors[staffId] || "")
      verify(warning.indexOf("until the app restarts") !== -1,
        "stored:false is a visible warning, not silence: " + warning)
      compare(controller.mergedForAccount("imap:work@example.com").length, 1,
        "the contacts are in use for this session either way")
    }

    function test_the_local_book_merges_without_a_backend_feature_gate() {
      mailService.localResult = [{ name: "Local", email: "local@example.org" }]
      mailService.backendCanCarddav = false
      controller.start()
      var calls = mailService.backendCalls.map(function(call) { return call.method })
      compare(JSON.stringify(calls), JSON.stringify(["contacts.suggest"]),
        "an old backend is asked only what it knows")
      var merged = controller.mergedForAccount("imap:work@example.com")
      compare(merged.length, 1)
      compare(merged[0].name, "Local")
      verify(!controller.connect(draft, "secret-1"),
        "connecting needs the CardDAV API")
    }

    function test_local_and_remote_merge_for_compose() {
      connectWork()
      controller.localBook = [{ name: "", email: "ivan@example.org" },
        { name: "Nearby", email: "near@example.org" }]
      var merged = controller.mergedForAccount("imap:work@example.com")
      compare(merged.length, 2)
      var ivan = merged.filter(function(c) { return c.email === "ivan@example.org" })[0]
      compare(ivan.name, "Иван Петров", "the remote name completes a bare local address")
    }

    // The controller reads its config through a watched FileView; the test
    // mock's signals stand in for the file's answers, driving the same
    // handlers the real file does.
    function configFileView() {
      for (var i = 0; i < controller.resources.length; i++) {
        var child = controller.resources[i]
        if (child && String(child).indexOf("FileView") !== -1) return child
      }
      return null
    }

    function test_a_config_reload_updates_settings_without_a_new_cycle() {
      var view = configFileView()
      verify(!!view, "the controller reads its config through a watched FileView")
      controller.store = CarddavSources.add(controller.store, {
        id: staffId, url: "https://dav.example.org/dav/card/", username: "employee@example.org",
        accountId: "imap:work@example.com", enabled: true,
        books: [{ href: "https://dav.example.org/dav/card/directory%40example.org/staff/",
          name: "CDM Staff", enabled: true }]
      })
      controller.startupStarted = true
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      mailService.refreshError = { code: -32000, message: "carddav_network_failed" }
      // A later answer from the watched file — an external edit, or the
      // write this controller's own saveStore just made. The store follows
      // the file, and no second snapshot/refresh cycle is stacked on top of
      // the targeted refresh the saving action already performed.
      var revision = controller.contactsRevision
      view.loaded()
      compare(controller.store.sources.length, 0,
        "the reload's settings answer replaces the store")
      verify(controller.contactsRevision > revision,
        "account-scoped bindings republish")
      compare(mailService.backendCalls.length, 0,
        "a config reload must not restart the pipeline")
      compare(controller.busy, false)
      // Hearing that the file is absent is not a change either: the store
      // resets and nothing runs.
      controller.store = CarddavSources.add(controller.store, {
        id: staffId, url: "https://dav.example.org/dav/card/", username: "employee@example.org",
        accountId: "", enabled: true, books: []
      })
      view.loadFailed()
      compare(controller.store.sources.length, 0)
      compare(mailService.backendCalls.length, 0,
        "an absent-file answer after startup runs nothing")
      compare(controller.loaded, true)
    }

    function test_the_initial_config_answer_starts_the_pipeline_once() {
      var view = configFileView()
      verify(!!view)
      mailService.localResult = [{ name: "Local", email: "local@example.org" }]
      // Nothing has answered yet: the file's first answer — loaded or
      // failed — is what starts the local book and the pipeline.
      controller.store = CarddavSources.emptyStore()
      controller.loaded = false
      controller.startupStarted = false
      view.loaded()
      compare(controller.loaded, true)
      compare(JSON.stringify(mailService.backendCalls.map(function(call) { return call.method })),
        JSON.stringify(["contacts.suggest"]),
        "the initial answer starts the one startup cycle")
      // The answer that follows must not start another.
      mailService.backendCalls = []
      view.loaded()
      view.loadFailed()
      compare(mailService.backendCalls.length, 0)
      // A first start whose file cannot be read still starts: the failed
      // answer marks the store known and begins the same flow.
      controller.loaded = false
      controller.startupStarted = false
      mailService.backendCalls = []
      view.loadFailed()
      compare(controller.loaded, true)
      compare(JSON.stringify(mailService.backendCalls.map(function(call) { return call.method })),
        JSON.stringify(["contacts.suggest"]),
        "an absent config still permits the local/start flow")
      compare(controller.mergedForAccount("imap:work@example.com").length, 1)
    }
  }
}
