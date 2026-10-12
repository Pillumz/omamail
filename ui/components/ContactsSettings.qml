import QtQuick
import qs.Commons
import qs.Ui
import "../contacts/CarddavSources.js" as CarddavSources

// The Contacts settings section: CardDAV address books the user connected,
// their books, and the connect form. Presentation only — the controller owns
// the flows and the backend owns the network.
Column {
  id: root

  required property var service
  required property var controller
  required property color textColor
  required property color dimColor
  required property color accentColor
  required property color urgentColor
  required property string panelFontFamily
  property bool adding: false
  property string formError: ""

  readonly property var sources: root.controller && root.controller.loaded
    && root.controller.store && Array.isArray(root.controller.store.sources)
    ? root.controller.store.sources : []

  function accountOptions() {
    var out = [{ value: "", label: "All accounts" }]
    var accounts = root.service && Array.isArray(root.service.accountSummaries)
      ? root.service.accountSummaries : []
    for (var i = 0; i < accounts.length; i++) {
      if (accounts[i] && accounts[i].id)
        out.push({ value: String(accounts[i].id), label: String(accounts[i].email || accounts[i].label || accounts[i].id) })
    }
    return out
  }

  component SettingRow: Rectangle {
    id: settingRow
    property string title: ""
    property string detail: ""
    default property alias controls: rowControls.data
    width: parent ? parent.width : 0
    implicitHeight: Math.max(settingText.implicitHeight, rowControls.implicitHeight) + Style.space(16)
    radius: Style.cornerRadius
    color: Style.normalFillFor(root.textColor, root.accentColor)
    Column {
      id: settingText
      anchors.left: parent.left
      anchors.leftMargin: Style.space(12)
      anchors.right: rowControls.left
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(2)
      Text {
        width: parent.width
        text: settingRow.title
        textFormat: Text.PlainText
        elide: Text.ElideMiddle
        color: root.textColor
        font.family: root.panelFontFamily
        font.pixelSize: Style.font.bodySmall
      }
      Text {
        width: parent.width
        visible: text !== ""
        text: settingRow.detail
        textFormat: Text.PlainText
        wrapMode: Text.WordWrap
        color: root.dimColor
        font.family: root.panelFontFamily
        font.pixelSize: Style.font.caption
      }
    }
    Row {
      id: rowControls
      anchors.right: parent.right
      anchors.rightMargin: Style.space(10)
      anchors.verticalCenter: parent.verticalCenter
      spacing: Style.space(6)
    }
  }

  component SectionHeading: Text {
    color: root.dimColor
    font.family: root.panelFontFamily
    font.pixelSize: Style.font.caption
    font.letterSpacing: 1
  }

  width: parent ? parent.width : implicitWidth
  spacing: Style.space(8)

  Column {
    width: parent.width
    spacing: Style.space(4)
    SectionHeading { text: "CONTACTS" }
    Text {
      width: parent.width
      text: "Recipient suggestions come from this device. Connect a CardDAV address book — a company directory, say — to suggest its contacts while composing. Address books are read-only here; nothing is written back to the server."
      color: root.dimColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.caption
      wrapMode: Text.WordWrap
      textFormat: Text.PlainText
    }
  }

  Text {
    width: parent.width
    visible: !!root.controller && !root.controller.available
    text: "Update the mail backend to connect CardDAV address books."
    textFormat: Text.PlainText
    wrapMode: Text.WordWrap
    color: root.dimColor
    font.family: root.panelFontFamily
    font.pixelSize: Style.font.caption
  }

  Repeater {
    model: root.sources

    Column {
      id: sourceBlock
      required property var modelData
      readonly property var source: modelData
      readonly property string error: root.controller
        ? String(root.controller.lastErrors[source.id] || "") : ""
      width: root.width
      spacing: Style.space(2)

      SettingRow {
        objectName: "carddav-source"
        title: CarddavSources.sourceLabel(sourceBlock.source)
        detail: (root.controller ? root.controller.accountLabel(sourceBlock.source) : "")
          + " · " + (root.controller ? root.controller.contactCount(sourceBlock.source) : 0)
          + " contacts"
          + (sourceBlock.error !== "" ? " · " + sourceBlock.error : "")
          + (root.controller && root.controller.busySourceId === sourceBlock.source.id ? " · Updating..." : "")

        IconTextButton {
          objectName: "carddav-source-refresh"
          anchors.verticalCenter: parent.verticalCenter
          visible: sourceBlock.source.enabled !== false
          text: "Refresh"
          bordered: false
          foreground: root.textColor
          fontFamily: root.panelFontFamily
          enabled: !!root.controller && root.controller.available && !root.controller.busy
          onClicked: root.controller.refreshSource(sourceBlock.source.id)
        }
        IconTextButton {
          objectName: "carddav-source-remove"
          anchors.verticalCenter: parent.verticalCenter
          text: "Remove"
          bordered: false
          foreground: root.urgentColor
          fontFamily: root.panelFontFamily
          enabled: !!root.controller && !root.controller.busy
          onClicked: root.controller.removeSource(sourceBlock.source.id)
        }
        ToggleSwitch {
          objectName: "carddav-source-toggle"
          anchors.verticalCenter: parent.verticalCenter
          checked: sourceBlock.source.enabled !== false
          interactive: !!root.controller && !root.controller.busy
          foreground: root.textColor
          accent: root.accentColor
          onToggled: if (root.controller)
            root.controller.setSourceEnabled(sourceBlock.source.id, sourceBlock.source.enabled === false)
        }
      }

      Repeater {
        model: sourceBlock.source.books || []
        delegate: SettingRow {
          id: bookRow
          required property var modelData
          objectName: "carddav-book"
          title: String(modelData.name || "Address book")
          detail: ""
          ToggleSwitch {
            objectName: "carddav-book-toggle"
            anchors.verticalCenter: parent.verticalCenter
            checked: bookRow.modelData.enabled !== false
            interactive: !!root.controller && !root.controller.busy && sourceBlock.source.enabled !== false
            foreground: root.textColor
            accent: root.accentColor
            onToggled: if (root.controller)
              root.controller.setBookEnabled(sourceBlock.source.id, bookRow.modelData.href,
                bookRow.modelData.enabled === false)
          }
        }
      }

      SettingRow {
        objectName: "carddav-source-account-row"
        title: "Suggest these contacts when writing from"
        detail: ""
        Dropdown {
          objectName: "carddav-source-account"
          width: Style.space(200)
          showLabel: false
          options: root.accountOptions()
          value: String(sourceBlock.source.accountId || "")
          foreground: root.textColor
          accent: root.accentColor
          fontFamily: root.panelFontFamily
          enabled: !!root.controller && !root.controller.busy
          onChanged: function(next) { root.controller.setSourceAccount(sourceBlock.source.id, next) }
        }
      }
    }
  }

  IconTextButton {
    objectName: "carddav-add"
    visible: !root.adding
    iconName: "plus"
    text: "Connect a CardDAV address book..."
    foreground: root.textColor
    fontFamily: root.panelFontFamily
    enabled: !!root.controller && root.controller.available && !root.controller.busy
    onClicked: {
      root.formError = ""
      root.adding = true
    }
  }

  Column {
    objectName: "carddav-add-form"
    width: parent.width
    visible: root.adding
    spacing: Style.space(6)

    TextField {
      id: sourceUrl
      objectName: "carddav-url"
      width: parent.width
      foreground: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      placeholderText: "Server URL, e.g. https://dav.example.com/dav/card/"
    }
    TextField {
      id: sourceUsername
      objectName: "carddav-username"
      width: parent.width
      foreground: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      placeholderText: "Username"
    }
    TextField {
      id: sourcePassword
      objectName: "carddav-password"
      width: parent.width
      password: true
      foreground: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      placeholderText: "Password or app password"
      onAccepted: root.connectSource()
    }
    TextField {
      id: sourceCollection
      objectName: "carddav-collection"
      width: parent.width
      foreground: root.textColor
      font.family: root.panelFontFamily
      font.pixelSize: Style.font.bodySmall
      placeholderText: "Address book URL (optional — used directly, skipping discovery)"
    }
    SettingRow {
      title: "Suggest these contacts when writing from"
      detail: ""
      Dropdown {
        id: sourceAccount
        objectName: "carddav-account"
        width: Style.space(200)
        showLabel: false
        options: root.accountOptions()
        value: ""
        foreground: root.textColor
        accent: root.accentColor
        fontFamily: root.panelFontFamily
      }
    }

    Row {
      spacing: Style.space(6)
      IconTextButton {
        objectName: "carddav-connect"
        text: root.controller && root.controller.busy ? "Connecting..." : "Connect"
        foreground: root.textColor
        accent: root.accentColor
        fontFamily: root.panelFontFamily
        enabled: !!root.controller && !root.controller.busy
        onClicked: root.connectSource()
      }
      IconTextButton {
        text: "Cancel"
        bordered: false
        foreground: root.dimColor
        fontFamily: root.panelFontFamily
        onClicked: root.adding = false
      }
    }
  }

  Text {
    objectName: "carddav-form-error"
    width: parent.width
    visible: text !== ""
    text: root.formError
    color: root.urgentColor
    font.family: root.panelFontFamily
    font.pixelSize: Style.font.caption
    wrapMode: Text.WordWrap
    textFormat: Text.PlainText
  }

  function connectSource() {
    root.formError = ""
    if (!root.controller) return
    var started = root.controller.connect({
      url: sourceUrl.text,
      username: sourceUsername.text,
      accountId: String(sourceAccount.value || ""),
      collection: sourceCollection.text
    }, sourcePassword.text)
    sourcePassword.text = ""
    if (!started && !root.controller.busy) root.formError = "The mail backend is busy; try again"
  }

  Connections {
    target: root.controller
    function onConnectFinished(ok, error) {
      if (!ok) { root.formError = error; return }
      root.formError = ""
      root.adding = false
      sourceUrl.text = ""
      sourceUsername.text = ""
      sourceCollection.text = ""
      sourceAccount.value = ""
    }
  }
}
