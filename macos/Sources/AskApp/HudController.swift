import AppKit
import SwiftTerm

final class HudController: NSObject, NSWindowDelegate, NSTextFieldDelegate, LocalProcessTerminalViewDelegate {
    private let environment: [String]
    private let window: NSWindow
    private let terminal: LocalProcessTerminalView
    private let prompt: NSTextField
    private let status: NSTextField
    private let toggleButton: NSButton
    private let headerBody: NSView
    private var running = false
    private var keyMonitor: Any?
    private var cwd: String = NSHomeDirectory()
    private var headerExpanded = false {
        didSet { applyHeaderVisibility(animated: true) }
    }

    init(environment: [String]) {
        self.environment = environment

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 780, height: 560),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        window.title = "Ask"
        window.minSize = NSSize(width: 480, height: 320)
        window.isReleasedWhenClosed = false
        if !window.setFrameUsingName("AskMain") {
            window.center()
        }
        window.setFrameAutosaveName("AskMain")
        self.window = window

        let font = NSFont.monospacedSystemFont(ofSize: 13, weight: .regular)
        self.terminal = LocalProcessTerminalView(frame: .zero, font: font, options: .default)
        self.prompt = NSTextField(string: "")
        self.status = NSTextField(labelWithString: "idle")
        self.toggleButton = NSButton(title: "Controls", target: nil, action: nil)
        self.headerBody = NSView()

        super.init()

        terminal.processDelegate = self
        window.delegate = self
        buildChrome()
        applyHeaderVisibility(animated: false)

        keyMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            self?.handleKey(event)
        }
    }

    deinit {
        if let keyMonitor {
            NSEvent.removeMonitor(keyMonitor)
        }
    }

    func show() {
        NSApp.activate(ignoringOtherApps: true)
        window.makeKeyAndOrderFront(nil)
        focusPrompt()
    }

    func hide() {
        window.orderOut(nil)
    }

    @objc func toggleControls() {
        headerExpanded.toggle()
    }

    func run(_ request: LaunchRequest) {
        if let requestCwd = request.cwd, !requestCwd.isEmpty {
            cwd = requestCwd
        }

        if request.clear {
            if terminal.process.running {
                terminal.terminate()
                setRunning(false)
            }
            clearScreen()
        } else if running {
            if let prompt = request.prompt, !prompt.isEmpty {
                sendToProcess(prompt + "\n")
            }
            return
        }

        let args = request.argv()
        guard request.startsProcess else { return }

        show()
        startAsk(args: args, focusTerminal: request.manage)
    }

    // MARK: - Chrome

    private func buildChrome() {
        guard let content = window.contentView else { return }

        let root = NSVisualEffectView()
        root.material = .contentBackground
        root.blendingMode = .behindWindow
        root.state = .active
        root.translatesAutoresizingMaskIntoConstraints = false
        content.addSubview(root)
        NSLayoutConstraint.activate([
            root.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            root.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            root.topAnchor.constraint(equalTo: content.topAnchor),
            root.bottomAnchor.constraint(equalTo: content.bottomAnchor),
        ])

        let header = makeHeader()
        header.translatesAutoresizingMaskIntoConstraints = false

        terminal.translatesAutoresizingMaskIntoConstraints = false
        terminal.wantsLayer = true
        terminal.layer?.cornerRadius = 8
        terminal.layer?.masksToBounds = true

        let caret = NSTextField(labelWithString: "❯")
        caret.font = NSFont.monospacedSystemFont(ofSize: 14, weight: .medium)
        caret.textColor = NSColor.controlAccentColor
        caret.translatesAutoresizingMaskIntoConstraints = false

        prompt.placeholderString = "Ask anything…"
        prompt.font = NSFont.systemFont(ofSize: 14)
        prompt.isBezeled = false
        prompt.isBordered = false
        prompt.drawsBackground = false
        prompt.focusRingType = .none
        prompt.delegate = self
        prompt.translatesAutoresizingMaskIntoConstraints = false

        let bar = NSView()
        bar.wantsLayer = true
        bar.layer?.backgroundColor = NSColor.separatorColor.withAlphaComponent(0.25).cgColor
        bar.layer?.cornerRadius = 8
        bar.translatesAutoresizingMaskIntoConstraints = false
        bar.addSubview(caret)
        bar.addSubview(prompt)

        root.addSubview(header)
        root.addSubview(terminal)
        root.addSubview(bar)

        NSLayoutConstraint.activate([
            header.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            header.trailingAnchor.constraint(equalTo: root.trailingAnchor),
            header.topAnchor.constraint(equalTo: root.topAnchor),

            terminal.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            terminal.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            terminal.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 8),
            terminal.bottomAnchor.constraint(equalTo: bar.topAnchor, constant: -10),

            bar.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 12),
            bar.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -12),
            bar.bottomAnchor.constraint(equalTo: root.bottomAnchor, constant: -12),
            bar.heightAnchor.constraint(equalToConstant: 36),

            caret.leadingAnchor.constraint(equalTo: bar.leadingAnchor, constant: 10),
            caret.centerYAnchor.constraint(equalTo: bar.centerYAnchor),

            prompt.leadingAnchor.constraint(equalTo: caret.trailingAnchor, constant: 8),
            prompt.trailingAnchor.constraint(equalTo: bar.trailingAnchor, constant: -10),
            prompt.centerYAnchor.constraint(equalTo: bar.centerYAnchor),
        ])
    }

    private func makeHeader() -> NSView {
        let wrap = NSView()

        toggleButton.target = self
        toggleButton.action = #selector(toggleControls)
        toggleButton.bezelStyle = .inline
        toggleButton.isBordered = false
        toggleButton.imagePosition = .imageLeading
        toggleButton.font = NSFont.systemFont(ofSize: 12, weight: .semibold)
        toggleButton.contentTintColor = NSColor.secondaryLabelColor
        toggleButton.translatesAutoresizingMaskIntoConstraints = false
        updateToggleChrome()

        status.font = NSFont.monospacedSystemFont(ofSize: 11, weight: .regular)
        status.textColor = NSColor.tertiaryLabelColor
        status.alignment = .right
        status.translatesAutoresizingMaskIntoConstraints = false

        let toggleRow = NSView()
        toggleRow.translatesAutoresizingMaskIntoConstraints = false
        toggleRow.addSubview(toggleButton)
        toggleRow.addSubview(status)

        let clear = smallButton(title: "Clear", action: #selector(clearTapped))
        let manage = smallButton(title: "Manage", action: #selector(manageTapped))
        let buttons = NSStackView(views: [clear, manage])
        buttons.orientation = .horizontal
        buttons.spacing = 8
        buttons.alignment = .centerY
        buttons.translatesAutoresizingMaskIntoConstraints = false

        let hint = NSTextField(wrappingLabelWithString: "Conversation stays in this window. ⌘K clear  ·  ⌘O manage  ·  esc hide")
        hint.font = NSFont.systemFont(ofSize: 11)
        hint.textColor = NSColor.secondaryLabelColor
        hint.translatesAutoresizingMaskIntoConstraints = false

        headerBody.translatesAutoresizingMaskIntoConstraints = false
        headerBody.addSubview(buttons)
        headerBody.addSubview(hint)

        var nextTop = hint.bottomAnchor
        if !ChildEnvironment.hasAPIKey(in: environment) {
            let warning = NSTextField(wrappingLabelWithString: "No API key in the environment. Put KEY=VALUE lines in ~/.config/ask.env")
            warning.font = NSFont.systemFont(ofSize: 11)
            warning.textColor = NSColor.systemOrange
            warning.translatesAutoresizingMaskIntoConstraints = false
            headerBody.addSubview(warning)
            NSLayoutConstraint.activate([
                warning.leadingAnchor.constraint(equalTo: headerBody.leadingAnchor, constant: 16),
                warning.trailingAnchor.constraint(equalTo: headerBody.trailingAnchor, constant: -16),
                warning.topAnchor.constraint(equalTo: hint.bottomAnchor, constant: 4),
            ])
            nextTop = warning.bottomAnchor
        }

        let rule = NSBox()
        rule.boxType = .separator
        rule.translatesAutoresizingMaskIntoConstraints = false

        wrap.addSubview(toggleRow)
        wrap.addSubview(headerBody)
        wrap.addSubview(rule)

        NSLayoutConstraint.activate([
            toggleRow.leadingAnchor.constraint(equalTo: wrap.leadingAnchor),
            toggleRow.trailingAnchor.constraint(equalTo: wrap.trailingAnchor),
            toggleRow.topAnchor.constraint(equalTo: wrap.topAnchor, constant: 4),
            toggleRow.heightAnchor.constraint(equalToConstant: 22),

            toggleButton.leadingAnchor.constraint(equalTo: toggleRow.leadingAnchor, constant: 12),
            toggleButton.centerYAnchor.constraint(equalTo: toggleRow.centerYAnchor),

            status.trailingAnchor.constraint(equalTo: toggleRow.trailingAnchor, constant: -14),
            status.centerYAnchor.constraint(equalTo: toggleRow.centerYAnchor),

            headerBody.leadingAnchor.constraint(equalTo: wrap.leadingAnchor),
            headerBody.trailingAnchor.constraint(equalTo: wrap.trailingAnchor),
            headerBody.topAnchor.constraint(equalTo: toggleRow.bottomAnchor, constant: 6),

            buttons.leadingAnchor.constraint(equalTo: headerBody.leadingAnchor, constant: 16),
            buttons.topAnchor.constraint(equalTo: headerBody.topAnchor),

            hint.leadingAnchor.constraint(equalTo: headerBody.leadingAnchor, constant: 16),
            hint.trailingAnchor.constraint(equalTo: headerBody.trailingAnchor, constant: -16),
            hint.topAnchor.constraint(equalTo: buttons.bottomAnchor, constant: 6),
            nextTop.constraint(equalTo: headerBody.bottomAnchor, constant: -8),

            rule.leadingAnchor.constraint(equalTo: wrap.leadingAnchor),
            rule.trailingAnchor.constraint(equalTo: wrap.trailingAnchor),
            rule.topAnchor.constraint(equalTo: headerBody.bottomAnchor),
            rule.bottomAnchor.constraint(equalTo: wrap.bottomAnchor),
        ])

        return wrap
    }

    private func smallButton(title: String, action: Selector) -> NSButton {
        let button = NSButton(title: title, target: self, action: action)
        button.bezelStyle = .rounded
        button.controlSize = .small
        return button
    }

    private func updateToggleChrome() {
        let name = headerExpanded ? "chevron.down" : "chevron.right"
        toggleButton.image = NSImage(systemSymbolName: name, accessibilityDescription: nil)
        toggleButton.title = "Controls"
        toggleButton.toolTip = headerExpanded ? "Hide controls" : "Show controls"
    }

    private func applyHeaderVisibility(animated: Bool) {
        let changes = {
            self.headerBody.isHidden = !self.headerExpanded
            self.updateToggleChrome()
            self.headerBody.superview?.layoutSubtreeIfNeeded()
        }
        if animated {
            NSAnimationContext.runAnimationGroup { ctx in
                ctx.duration = 0.15
                ctx.allowsImplicitAnimation = true
                changes()
            }
        } else {
            changes()
        }
    }

    // MARK: - Process

    private func askBinary() -> String {
        let bundled = (Bundle.main.bundlePath as NSString)
            .appendingPathComponent("Contents/Helpers/ask")
        if FileManager.default.isExecutableFile(atPath: bundled) {
            return bundled
        }
        for fallback in ["/usr/local/bin/ask", "/opt/homebrew/bin/ask", "/bin/ask"] {
            if FileManager.default.isExecutableFile(atPath: fallback) {
                return fallback
            }
        }
        return bundled
    }

    private func startAsk(args: [String], focusTerminal: Bool) {
        let executable = askBinary()
        guard FileManager.default.isExecutableFile(atPath: executable) else {
            feed("ask binary not found. build with macos/build-app.sh\r\n")
            return
        }

        setRunning(true)
        terminal.startProcess(
            executable: executable,
            args: args,
            environment: environment,
            execName: "ask",
            currentDirectory: cwd
        )
        if focusTerminal {
            window.makeFirstResponder(terminal)
        } else {
            focusPrompt()
        }
    }

    private func sendToProcess(_ text: String) {
        terminal.process.send(data: Array(text.utf8)[...])
    }

    private func setRunning(_ value: Bool) {
        running = value
        status.stringValue = value ? "running" : "idle"
        status.textColor = value
            ? NSColor.controlAccentColor
            : NSColor.tertiaryLabelColor
    }

    private func focusPrompt() {
        window.makeFirstResponder(prompt)
        if let editor = prompt.currentEditor() {
            editor.selectedRange = NSRange(location: prompt.stringValue.count, length: 0)
        }
    }

    private func feed(_ text: String) {
        terminal.feed(text: text)
    }

    private func clearScreen() {
        terminal.getTerminal().resetToInitialState()
        terminal.clearScrollback()
        terminal.needsDisplay = true
    }

    // MARK: - Input

    @objc private func clearTapped() {
        run(LaunchRequest(clear: true))
    }

    @objc private func manageTapped() {
        run(LaunchRequest(manage: true))
    }

    private func handleKey(_ event: NSEvent) -> NSEvent? {
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        let chars = event.charactersIgnoringModifiers ?? ""

        if event.keyCode == 53 {
            hide()
            return nil
        }

        if flags == .command {
            switch chars {
            case "w":
                hide()
                return nil
            case "k":
                run(LaunchRequest(clear: true))
                return nil
            case "o":
                run(LaunchRequest(manage: true))
                return nil
            default:
                break
            }
        }
        return event
    }

    func control(_ control: NSControl, textView: NSTextView, doCommandBy commandSelector: Selector) -> Bool {
        if commandSelector == #selector(NSResponder.insertNewline(_:)) {
            submitPrompt()
            return true
        }
        if commandSelector == #selector(NSResponder.cancelOperation(_:)) {
            hide()
            return true
        }
        return false
    }

    private func submitPrompt() {
        let text = prompt.stringValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        prompt.stringValue = ""

        if running {
            sendToProcess(text + "\n")
            return
        }

        run(parseSlash(text))
    }

    private func parseSlash(_ text: String) -> LaunchRequest {
        if text == "/c" || text == "/clear" {
            return LaunchRequest(clear: true)
        }
        if text == "/o" || text == "/manage" {
            return LaunchRequest(manage: true)
        }
        if text.hasPrefix("/i ") {
            return LaunchRequest(prompt: String(text.dropFirst(3)), image: true)
        }
        return LaunchRequest(prompt: text)
    }

    // MARK: - Delegates

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        hide()
        return false
    }

    func sizeChanged(source: LocalProcessTerminalView, newCols: Int, newRows: Int) {}

    func setTerminalTitle(source: LocalProcessTerminalView, title: String) {}

    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {
        if let directory, !directory.isEmpty {
            cwd = directory
        }
    }

    func processTerminated(source: TerminalView, exitCode: Int32?) {
        setRunning(false)
        if let exitCode, exitCode != 0 {
            feed("\r\n  [exit \(exitCode)]\r\n")
        } else {
            feed("\r\n")
        }
        focusPrompt()
    }
}
