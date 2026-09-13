import QtQuick
import Quickshell
import Quickshell.Services.SystemTray
import qs.Commons
import qs.Ui as Ui

Ui.BarWidget {
    id: root
    moduleName: "dev.neoism.agent-status"
    // One layout entry, one native button per process. Never collapse another
    // running instance, infer activity from strings, or activate on appearance.
    readonly property var items: SystemTray.items.values.filter(item => item.id === "neoism")
    visible: items.length > 0
    implicitWidth: items.length === 0 ? 0 : (vertical ? barSize : items.length * Style.bar.iconSlot)
    implicitHeight: items.length === 0 ? 0 : (vertical ? items.length * Style.bar.iconSlot : barSize)

    Repeater {
        model: root.items
        delegate: Ui.BarIconButton {
            id: button
            required property var modelData
            required property int index
            bar: root.bar
            x: root.vertical ? 0 : index * implicitWidth
            y: root.vertical ? index * implicitHeight : 0
            useActiveColor: false
            tooltipText: [modelData.tooltipTitle, modelData.tooltipDescription].filter(text => !!text).join("\n")
            iconComponent: Component {
                Image {
                    anchors.fill: parent
                    source: button.modelData.icon
                    fillMode: Image.PreserveAspectFit
                    // SNI IconPixmap is an image-provider URI; request physical
                    // pixels, matching the native tray's HiDPI Image path.
                    sourceSize.width: Math.round(Math.min(width, height) * Screen.devicePixelRatio)
                    sourceSize.height: sourceSize.width
                    cache: false
                }
            }
            onPressed: mouseButton => {
                if (mouseButton === Qt.LeftButton) modelData.activate()
            }
        }
    }
}
