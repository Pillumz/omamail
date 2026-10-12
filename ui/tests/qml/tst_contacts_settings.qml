import QtQuick 2.15
import QtTest 1.3
import qs.Commons as Commons
import "../../components" as Omamail
import "../../contacts" as Contacts

// The Contacts settings row against a live controller. The row's source comes
// through the Repeater's modelData, where the books list is a QVariantList —
// the contact count must still be counted, not silently stay zero.
Item {
  width: 800
  height: 500

  QtObject {
    id: mailService
    property bool backendCanCarddav: true
    property var accountSummaries: [{ id: "imap:work@example.com", email: "work@example.com" }]
    property var backend: ({ ready: true, call: function(method, params, callback) {
      if (method === "contacts.suggest") { callback([], null); return }
      if (method === "carddav.discover") { callback({ principal: "https://dav.example.org/p/", books: [
        { href: "https://dav.example.org/a/", name: "A", enabled: true },
        { href: "https://dav.example.org/b/", name: "B", enabled: true }
      ] }, null); return }
      if (method === "carddav.refresh") { callback({
        stored: true, errors: [],
        books: [
          { href: "https://dav.example.org/a/", name: "A", contacts: [{ name: "One", email: "one@x" }] },
          { href: "https://dav.example.org/b/", name: "B", contacts: [{ name: "Two", email: "two@x" }, { name: "Three", email: "three@x" }] }
        ]
      }, null); return }
      if (method === "carddav.snapshot") { callback({ books: [] }, null); return }
      callback(null, { code: -32601, message: "unknown_method" })
    } })
    function configPath(name) { return "/tmp/omamail-test/" + name }
    function writeConfig(name, payload, callback) { callback(true, "") }
    function credentialPut(kind, accountId, label, secret, callback) { callback(true, "") }
    function credentialDelete(kind, accountId, label, callback) { callback(true, "") }
  }

  Contacts.CarddavController {
    id: controller
    service: mailService
  }

  Omamail.ContactsSettings {
    id: settings
    width: 700
    service: mailService
    controller: controller
    textColor: Commons.Color.foreground
    dimColor: Commons.Color.foreground
    accentColor: Commons.Color.accent
    urgentColor: Commons.Color.accent
    panelFontFamily: "Sans"
  }

  TestCase {
    name: "ContactsSettings"
    when: windowShown

    function sourceRow() {
      var found = null
      var stack = [settings]
      while (stack.length > 0) {
        var item = stack.pop()
        if (item.objectName === "carddav-source") found = item
        for (var i = 0; i < item.children.length; i++) stack.push(item.children[i])
      }
      return found
    }

    function test_the_row_counts_contacts_through_the_repeater() {
      controller.loaded = true
      controller.connect({ url: "https://dav.example.org/", username: "u", collection: "", accountId: "" }, "secret")
      tryCompare(controller, "busy", false, 4000)
      var row = sourceRow()
      verify(row !== null, "the connected source has a row")
      tryCompare(row, "detail", "All accounts · 3 contacts", 2000)
    }
  }
}
