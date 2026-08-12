import AppKit

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var hud: HudController!
    private var env: [String] = []

    func applicationDidFinishLaunching(_ notification: Notification) {
        installMenu()
        env = ChildEnvironment.variables()
        hud = HudController(environment: env)
        hud.show()

        let launch = LaunchRequest.fromCommandLine(CommandLine.arguments)
        if launch.startsProcess {
            hud.run(launch)
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        hud.show()
        return true
    }

    func application(_ application: NSApplication, open urls: [URL]) {
        hud.show()
        for url in urls {
            let request = LaunchRequest.fromURL(url)
            if request.startsProcess {
                hud.run(request)
            }
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    private func installMenu() {
        let main = NSMenu()

        let appMenuItem = NSMenuItem()
        main.addItem(appMenuItem)
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "Hide Ask", action: #selector(NSApplication.hide(_:)), keyEquivalent: "h")
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "Quit Ask", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appMenuItem.submenu = appMenu

        let editItem = NSMenuItem()
        main.addItem(editItem)
        let edit = NSMenu(title: "Edit")
        edit.addItem(withTitle: "Cut", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        edit.addItem(withTitle: "Copy", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        edit.addItem(withTitle: "Paste", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        edit.addItem(withTitle: "Select All", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editItem.submenu = edit

        let viewItem = NSMenuItem()
        main.addItem(viewItem)
        let view = NSMenu(title: "View")
        let controls = NSMenuItem(title: "Controls", action: #selector(toggleControls), keyEquivalent: "/")
        controls.target = self
        view.addItem(controls)
        viewItem.submenu = view

        let windowItem = NSMenuItem()
        main.addItem(windowItem)
        let windowMenu = NSMenu(title: "Window")
        let close = NSMenuItem(title: "Close", action: #selector(hideHud), keyEquivalent: "w")
        close.target = self
        windowMenu.addItem(close)
        let clear = NSMenuItem(title: "Clear conversation", action: #selector(clearHud), keyEquivalent: "k")
        clear.target = self
        windowMenu.addItem(clear)
        windowItem.submenu = windowMenu

        NSApp.mainMenu = main
    }

    @objc private func hideHud() {
        hud.hide()
    }

    @objc private func clearHud() {
        hud.run(LaunchRequest(clear: true))
    }

    @objc private func toggleControls() {
        hud.toggleControls()
    }
}
