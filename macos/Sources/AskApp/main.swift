import AppKit

private let appDelegate = AppDelegate()

let app = NSApplication.shared
app.delegate = appDelegate
app.setActivationPolicy(.regular)
app.run()
