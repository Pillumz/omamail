import QtQuick 2.15
import QtTest 1.3
import "../../contacts" as Omamail
import "../../contacts/CarddavSources.js" as CarddavSources

Item {
  width: 400
  height: 300

  // A real QObject property so the controller's readiness connection fires
  // the way the backend's does; tests toggle it to drive startup/reconnect.
  QtObject {
    id: mockBackend
    property bool ready: true
    // Snapshot and refresh answers the test releases itself, to observe
    // in-flight order and what a retired backend life may no longer publish.
    property bool holdSnapshots: false
    property var snapshotReleases: []
    property bool holdRefreshes: false
    property var refreshReleases: []

    function call(method, params, callback) {
      mailService.backendCalls.push({ method: method, params: params })
      if (method === "contacts.suggest") { callback(mailService.localResult, null); return }
      if (method === "carddav.discover") { callback(mailService.discoverResult, mailService.discoverError); return }
      if (method === "carddav.refresh") {
        if (mockBackend.holdRefreshes) { mockBackend.refreshReleases.push(callback); return }
        callback(mailService.refreshResult, mailService.refreshError)
        return
      }
      if (method === "carddav.snapshot") {
        if (mockBackend.holdSnapshots) { mockBackend.snapshotReleases.push(callback); return }
        callback(mailService.snapshotResult || { books: [] }, null)
        return
      }
      if (method === "carddav.forget") { callback({ forgotten: true }, mailService.forgetError); return }
      callback(null, { code: -32601, message: "unknown_method" })
    }
  }

  QtObject {
    id: mailService

    property bool backendCanCarddav: true
    property var backend: mockBackend
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
    property bool failConfigWrites: false
    property bool holdConfigWrites: false
    property var configWriteReleases: []
    property string credentialDeleteError: ""
    property var forgetError: null

    function configPath(name) { return "/tmp/omamail-test/" + name }
    function writeConfig(name, payload, callback) {
      configWrites.push({ name: name, payload: payload })
      if (holdConfigWrites) { configWriteReleases.push(callback); return }
      callback(!failConfigWrites, failConfigWrites ? "Disk full" : "")
    }
    function credentialPut(kind, accountId, clientId, secret, callback) {
      credentialCalls.push({ op: "put", kind: kind, id: accountId, secret: secret })
      callback(true, "")
      return true
    }
    function credentialDelete(kind, accountId, clientId, callback) {
      credentialCalls.push({ op: "delete", kind: kind, id: accountId })
      // The credentials API reports a missing credential as a successful
      // delete, so the controller sees only this one outcome.
      callback(credentialDeleteError === "", credentialDeleteError)
      return true
    }
  }

  Omamail.CarddavController {
    id: controller
    service: mailService
    loaded: true
  }

  SignalSpy { id: removedSpy; target: controller; signalName: "sourceRemoved" }

  TestCase {
    name: "CarddavController"

    // The Rust known-answer vector for (https://dav.example.org/dav/card/,
    // employee@example.org): the UI must derive the id the backend derives.
    property string staffId: "carddav:9ff5d0d63e48ae4dd33ec08f3adbaa748bd131b7f320641e9438adf4c7e8f119"
    property string staffBookHref: "https://dav.example.org/dav/card/directory%40example.org/staff/"
    property var draft: ({ url: "https://dav.example.org/dav/card/",
      username: "employee@example.org", accountId: "imap:work@example.com" })

    function init() {
      // Retire anything left mid-flight before the mock's readiness signal
      // can start a cycle from a previous test's store.
      controller.loaded = false
      if (!mockBackend.ready) mockBackend.ready = true
      mockBackend.holdSnapshots = false
      mockBackend.snapshotReleases = []
      mockBackend.holdRefreshes = false
      mockBackend.refreshReleases = []
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
      mailService.failConfigWrites = false
      mailService.holdConfigWrites = false
      mailService.configWriteReleases = []
      mailService.credentialDeleteError = ""
      mailService.forgetError = null
      controller.store = CarddavSources.emptyStore()
      controller.snapshots = ({})
      controller.lastErrors = ({})
      controller.pendingQueue = []
      controller.cycleRunning = false
      controller.cycleGeneration = 0
      controller.busy = false
      controller.busySourceId = ""
      controller.localBook = []
      removedSpy.clear()
      controller.loaded = true
      controller.startupStarted = true
    }

    function connectWork() {
      mailService.discoverResult = { books: [
        { href: staffBookHref, name: "CDM Staff" }
      ] }
      mailService.refreshResult = { books: [
        { href: staffBookHref, contacts: [{ name: "Иван Петров", email: "ivan@example.org" }] }
      ], errors: [], stored: true }
      verify(controller.connect(draft, "secret-1"))
      compare(controller.busy, false)
    }

    function sourceRecord() {
      return {
        id: staffId, url: "https://dav.example.org/dav/card/", username: "employee@example.org",
        accountId: "imap:work@example.com", enabled: true,
        books: [{ href: staffBookHref, name: "CDM Staff", enabled: true }]
      }
    }

    function storeWithSource() {
      return CarddavSources.add(CarddavSources.emptyStore(), sourceRecord())
    }

    function backendCallsOf(method) {
      return mailService.backendCalls.filter(function(call) { return call.method === method })
    }

    function releaseSnapshot() {
      verify(mockBackend.snapshotReleases.length > 0, "a snapshot answer is waiting")
      var callback = mockBackend.snapshotReleases.shift()
      callback(mailService.snapshotResult || { books: [] }, null)
    }

    function releaseRefresh() {
      verify(mockBackend.refreshReleases.length > 0, "a refresh answer is waiting")
      var callback = mockBackend.refreshReleases.shift()
      callback(mailService.refreshResult, mailService.refreshError)
    }

    function releaseConfigWrite(ok) {
      verify(mailService.configWriteReleases.length > 0, "a config write is waiting")
      var callback = mailService.configWriteReleases.shift()
      callback(ok !== false, ok !== false ? "" : "Disk full")
    }

    function seedContacts() {
      var books = {}
      books[staffBookHref] = [{ name: "Kept", email: "kept@example.org" }]
      controller.applySnapshot(staffId, books)
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
      verify(failed.indexOf("could not be removed") === -1,
        "a successful cleanup is not reported as a failure: " + failed)
      compare(JSON.stringify(mailService.credentialCalls.map(function(call) { return call.op + ":" + call.id })),
        JSON.stringify(["put:" + CarddavSources.deriveId("https://other.example.org/card/", "bob"),
                        "delete:" + CarddavSources.deriveId("https://other.example.org/card/", "bob")]),
        "a failed connect removes the password it stored, and only that one")
      compare(controller.store.sources.length, 1, "the working source stays connected")
    }

    function test_a_failed_connect_discloses_a_password_it_could_not_remove() {
      mailService.discoverResult = null
      mailService.discoverError = { code: -32000, message: "carddav_auth_refused" }
      mailService.credentialDeleteError = "credential_store_unavailable"
      var failed = ""
      var ok = true
      var spy = function(good, error) { ok = good; failed = error }
      controller.connectFinished.connect(spy)
      var draft = { url: "https://cleanup.example.org/card/", username: "bob", accountId: "" }
      verify(controller.connect(draft, "secret-3"))
      controller.connectFinished.disconnect(spy)
      compare(ok, false)
      verify(failed.indexOf("refused") !== -1, "the connect failure is reported: " + failed)
      verify(failed.indexOf("could not be removed") !== -1,
        "the leftover password is disclosed, not swallowed: " + failed)
      verify(failed.indexOf("credential_store_unavailable") === -1,
        "the cleanup failure is said in words, not as a code: " + failed)
      // The same connect, once the keyring answers, retries and completes
      // the cleanup the first attempt could not.
      mailService.credentialDeleteError = ""
      mailService.credentialCalls = []
      var retryOk = true
      var retryError = ""
      var retrySpy = function(good, error) { retryOk = good; retryError = error }
      controller.connectFinished.connect(retrySpy)
      verify(controller.connect(draft, "secret-3"))
      controller.connectFinished.disconnect(retrySpy)
      compare(retryOk, false)
      verify(retryError.indexOf("could not be removed") === -1,
        "the retry's cleanup succeeded: " + retryError)
      compare(JSON.stringify(mailService.credentialCalls.map(function(call) { return call.op })),
        JSON.stringify(["put", "delete"]),
        "the retry stores again and removes what it stored")
    }

    function test_a_connect_whose_save_failed_removes_the_password_it_stored() {
      mailService.discoverResult = { books: [{ href: "https://cleanup.example.org/book/", name: "Cleanup" }] }
      mailService.failConfigWrites = true
      var failed = ""
      var ok = true
      var spy = function(good, error) { ok = good; failed = error }
      var draft = { url: "https://cleanup.example.org/card/", username: "bob", accountId: "" }
      controller.connectFinished.connect(spy)
      verify(controller.connect(draft, "secret-4"))
      controller.connectFinished.disconnect(spy)
      compare(ok, false)
      verify(failed.length > 0, "the save failure is reported: " + failed)
      compare(JSON.stringify(mailService.credentialCalls.map(function(call) { return call.op })),
        JSON.stringify(["put", "delete"]),
        "the password stored for a connect that could not be saved is deleted")
      compare(controller.store.sources.length, 0)
      // When even that delete fails, the leftover password is disclosed.
      mailService.credentialCalls = []
      mailService.credentialDeleteError = "credential_store_unavailable"
      var failed2 = ""
      var spy2 = function(good, error) { failed2 = error }
      controller.connectFinished.connect(spy2)
      verify(controller.connect(draft, "secret-4"))
      controller.connectFinished.disconnect(spy2)
      verify(failed2.indexOf("could not be removed") !== -1,
        "a failed save cleanup is disclosed: " + failed2)
      verify(failed2.indexOf("credential_store_unavailable") === -1,
        "the cleanup failure is said in words, not as a code: " + failed2)
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
        { href: staffBookHref, contacts: [{ name: "Earlier", email: "earlier@example.org" }] }
      ] }
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      mailService.refreshError = { code: -32000, message: "carddav_auth_refused" }
      controller.store = storeWithSource()
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
        { href: staffBookHref, contacts: [{ name: "Иван Петров", email: "ivan@example.org" }] }
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
      controller.store = storeWithSource()
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

    function test_ready_starts_one_cycle_and_refuses_a_second_trigger() {
      mockBackend.holdSnapshots = true
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      controller.store = storeWithSource()
      controller.loaded = true
      controller.startupStarted = true
      mockBackend.ready = false
      mockBackend.ready = true
      compare(backendCallsOf("carddav.snapshot").length, 1,
        "one readiness event starts one snapshot load")
      // A completing config answer or any other caller while the answer is
      // in flight must not stack a second pipeline on top.
      controller.start()
      compare(backendCallsOf("carddav.snapshot").length, 1,
        "a second start while the answer is in flight is refused")
      releaseSnapshot()
      compare(backendCallsOf("carddav.snapshot").length, 1)
      compare(backendCallsOf("carddav.refresh").length, 1,
        "the cycle moved on to the refresh exactly once")
      compare(controller.cycleRunning, false)
      compare(controller.busy, false)
    }

    function test_stored_snapshots_load_before_any_refresh_one_at_a_time() {
      mockBackend.holdSnapshots = true
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      var second = {
        id: CarddavSources.deriveId("https://other.example.org/dav/card/", "bob"),
        url: "https://other.example.org/dav/card/", username: "bob",
        accountId: "", enabled: true,
        books: [{ href: "https://other.example.org/dav/card/book/", name: "Other", enabled: true }]
      }
      controller.store = CarddavSources.add(storeWithSource(), second)
      controller.loaded = true
      controller.startupStarted = true
      mockBackend.ready = false
      mockBackend.ready = true
      compare(backendCallsOf("carddav.snapshot").length, 1, "snapshots are serialized")
      compare(backendCallsOf("carddav.refresh").length, 0, "no refresh starts before snapshots finish")
      releaseSnapshot()
      compare(backendCallsOf("carddav.snapshot").length, 2, "the next snapshot follows the first answer")
      compare(backendCallsOf("carddav.refresh").length, 0, "still no overlapping refresh")
      releaseSnapshot()
      compare(backendCallsOf("carddav.refresh").length, 2, "each enabled source refreshes once, after snapshots")
      compare(controller.cycleRunning, false)
      compare(controller.busy, false)
    }

    function test_a_reconnect_drops_an_answer_from_the_previous_backend_life() {
      mockBackend.holdSnapshots = true
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      controller.store = storeWithSource()
      controller.loaded = true
      controller.startupStarted = true
      mockBackend.ready = false
      mockBackend.ready = true
      compare(backendCallsOf("carddav.snapshot").length, 1)
      // The backend drops while that answer is still on its way.
      mockBackend.ready = false
      compare(controller.cycleRunning, false, "a dead backend retires the cycle")
      compare(controller.busy, false)
      mockBackend.ready = true
      compare(backendCallsOf("carddav.snapshot").length, 2,
        "a reconnect starts one fresh snapshot load")
      mailService.snapshotResult = { books: [
        { href: staffBookHref, contacts: [{ name: "Old", email: "old@example.org" }] }
      ] }
      releaseSnapshot()
      compare(controller.mergedForAccount("imap:work@example.com").length, 0,
        "the retired life's answer publishes nothing")
      compare(controller.cycleRunning, true, "the retired answer does not advance the new cycle")
      mailService.snapshotResult = { books: [
        { href: staffBookHref, contacts: [{ name: "New", email: "new@example.org" }] }
      ] }
      releaseSnapshot()
      tryCompare(controller, "cycleRunning", false, 2000)
      var merged = controller.mergedForAccount("imap:work@example.com")
      compare(merged.length, 1)
      compare(merged[0].email, "new@example.org")
    }

    function test_a_dead_backend_drops_queued_sources() {
      mockBackend.holdSnapshots = true
      var second = {
        id: CarddavSources.deriveId("https://other.example.org/dav/card/", "bob"),
        url: "https://other.example.org/dav/card/", username: "bob",
        accountId: "", enabled: true,
        books: [{ href: "https://other.example.org/dav/card/book/", name: "Other", enabled: true }]
      }
      controller.store = CarddavSources.add(storeWithSource(), second)
      controller.loaded = true
      controller.startupStarted = true
      mockBackend.ready = false
      mockBackend.ready = true
      compare(backendCallsOf("carddav.snapshot").length, 1, "the first snapshot is in flight")
      compare(controller.pendingQueue.length, 1, "the second source waits in the queue")
      mockBackend.ready = false
      compare(controller.pendingQueue.length, 0, "a dead backend leaves no queued work behind")
      compare(controller.cycleRunning, false)
      compare(controller.busy, false)
    }

    function test_a_disconnect_drops_a_stale_manual_refresh_answer() {
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      controller.store = storeWithSource()
      seedContacts()
      mockBackend.holdRefreshes = true
      verify(controller.refreshSource(staffId))
      compare(controller.busy, true, "the refresh is in flight")
      compare(backendCallsOf("carddav.refresh").length, 1)
      mockBackend.ready = false
      compare(controller.busy, true, "a pending transaction keeps the controller busy while it is unresolved")
      mailService.refreshResult = { books: [
        { href: staffBookHref, contacts: [{ name: "Stale", email: "stale@example.org" }] }
      ], errors: [], stored: true }
      releaseRefresh()
      var merged = controller.mergedForAccount("imap:work@example.com")
      compare(merged.length, 1)
      compare(merged[0].email, "kept@example.org",
        "the retired life's manual answer publishes nothing")
      compare(controller.busy, false, "the stale completion still releases the operation")
      verify(controller.lastErrors[staffId] === undefined,
        "a retired answer reports no new error on the source")
    }

    function test_a_retired_life_does_not_publish_a_stale_book_list() {
      mailService.discoverResult = { books: [
        { href: "https://dav.example.org/other/", name: "Other" }
      ] }
      controller.store = storeWithSource()
      mailService.holdConfigWrites = true
      verify(controller.refreshSource(staffId))
      compare(controller.busy, true)
      compare(mailService.configWriteReleases.length, 1, "the discovered book list waits to be saved")
      mockBackend.ready = false
      releaseConfigWrite(true)
      compare(controller.store.sources.length, 1)
      compare(controller.store.sources[0].books.length, 1)
      compare(controller.store.sources[0].books[0].href, staffBookHref,
        "a write from the retired life does not replace the stored books")
      compare(controller.busy, false)
    }

    function test_a_source_removed_with_an_answer_in_flight_stays_removed() {
      mockBackend.holdSnapshots = true
      mailService.discoverError = { code: -32000, message: "carddav_network_failed" }
      controller.store = storeWithSource()
      controller.loaded = true
      controller.startupStarted = true
      mockBackend.ready = false
      mockBackend.ready = true
      compare(backendCallsOf("carddav.snapshot").length, 1)
      verify(controller.removeSource(staffId),
        "the row can be removed while its snapshot answer is in flight")
      compare(controller.store.sources.length, 0)
      compare(removedSpy.count, 1)
      mailService.snapshotResult = { books: [
        { href: staffBookHref, contacts: [{ name: "Ghost", email: "ghost@example.org" }] }
      ] }
      releaseSnapshot()
      compare(controller.snapshots[staffId] === undefined, true,
        "the late answer does not rebuild a removed source")
      compare(controller.mergedForAccount("imap:work@example.com").length, 0)
      compare(controller.cycleRunning, false, "the cycle still finishes")
    }

    function test_removal_is_refused_while_the_backend_cannot_forget_the_cache() {
      controller.store = storeWithSource()
      seedContacts()
      mailService.backendCanCarddav = false
      verify(!controller.removeSource(staffId))
      compare(controller.store.sources.length, 1, "the row's source is still configured")
      compare(removedSpy.count, 0, "no removal is claimed")
      verify(controller.snapshots[staffId] !== undefined, "the visible contacts stay")
      verify(String(controller.lastErrors[staffId] || "").indexOf("backend") !== -1,
        "the row explains the backend is not ready: " + controller.lastErrors[staffId])
      compare(mailService.credentialCalls.length, 0, "the password is not touched")
      compare(backendCallsOf("carddav.forget").length, 0)
      compare(mailService.configWrites.length, 0)
    }

    function test_a_refused_credential_delete_keeps_the_source_and_reports() {
      controller.store = storeWithSource()
      seedContacts()
      mailService.credentialDeleteError = "credential_store_unavailable"
      verify(controller.removeSource(staffId))
      compare(controller.store.sources.length, 1, "the row's source is still configured")
      compare(removedSpy.count, 0, "no removal is claimed")
      verify(controller.snapshots[staffId] !== undefined, "the visible contacts stay")
      verify(String(controller.lastErrors[staffId] || "").indexOf("keyring") !== -1,
        "the failure is visible on the row")
      compare(backendCallsOf("carddav.forget").length, 0,
        "the cache is not touched before the password is gone")
      compare(mailService.configWrites.length, 0, "the config still names the source")
      compare(controller.busy, false)
    }

    function test_a_failed_forget_keeps_the_source_and_a_retry_removes_it() {
      controller.store = storeWithSource()
      seedContacts()
      mailService.forgetError = { code: -32000, message: "backend_unavailable" }
      verify(controller.removeSource(staffId))
      compare(controller.store.sources.length, 1, "the row's source is still configured")
      compare(removedSpy.count, 0, "no removal is claimed")
      verify(controller.snapshots[staffId] !== undefined, "the visible contacts stay")
      compare(mailService.configWrites.length, 0, "the config still names the source")
      verify(String(controller.lastErrors[staffId] || "").length > 0)
      // The password was deleted first; the retry finds it already gone
      // (a successful delete to the credentials API) and finishes the rest.
      mailService.forgetError = null
      verify(controller.removeSource(staffId))
      compare(controller.store.sources.length, 0)
      compare(removedSpy.count, 1)
      verify(controller.snapshots[staffId] === undefined)
      verify(controller.lastErrors[staffId] === undefined)
    }

    function test_a_failed_config_save_keeps_the_source_and_a_retry_removes_it() {
      controller.store = storeWithSource()
      seedContacts()
      mailService.failConfigWrites = true
      verify(controller.removeSource(staffId))
      compare(controller.store.sources.length, 1, "the row's source is still configured")
      compare(removedSpy.count, 0, "no removal is claimed")
      verify(controller.snapshots[staffId] !== undefined, "the visible contacts stay")
      verify(String(controller.lastErrors[staffId] || "") === "Disk full")
      mailService.failConfigWrites = false
      verify(controller.removeSource(staffId))
      compare(controller.store.sources.length, 0)
      compare(removedSpy.count, 1)
      verify(controller.snapshots[staffId] === undefined)
      verify(controller.lastErrors[staffId] === undefined)
    }
  }
}