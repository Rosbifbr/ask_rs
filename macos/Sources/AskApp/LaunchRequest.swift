import Foundation

struct LaunchRequest {
    var prompt: String?
    var image = false
    var manage = false
    var clear = false
    var cwd: String?

    var startsProcess: Bool {
        manage || clear || !(prompt?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
    }

    static func fromCommandLine(_ args: [String]) -> LaunchRequest {
        var req = LaunchRequest()
        var parts: [String] = []
        for arg in args.dropFirst() {
            switch arg {
            case "-i", "--image": req.image = true
            case "-o", "--manage": req.manage = true
            case "-c", "--clear": req.clear = true
            default:
                if !arg.hasPrefix("-") {
                    parts.append(arg)
                }
            }
        }
        if !parts.isEmpty {
            req.prompt = parts.joined(separator: " ")
        }
        return req
    }

    static func fromURL(_ url: URL) -> LaunchRequest {
        var req = LaunchRequest()
        let action = (url.host ?? url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/")))
            .lowercased()
        let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems ?? []

        func item(_ name: String) -> String? {
            items.first(where: { $0.name == name })?.value
        }

        func flag(_ names: String...) -> Bool {
            names.contains { name in
                guard let value = item(name) else { return false }
                return value == "1" || value.lowercased() == "true" || value.lowercased() == "yes"
            }
        }

        req.prompt = item("text") ?? item("q") ?? item("prompt")
        req.image = flag("i", "image")
        req.cwd = item("cwd")

        switch action {
        case "clear", "c":
            req.clear = true
        case "manage", "o":
            req.manage = true
        default:
            break
        }
        return req
    }

    func argv() -> [String] {
        var args: [String] = []
        if image { args.append("-i") }
        if manage { args.append("-o") }
        if clear { args.append("-c") }
        if let prompt, !prompt.isEmpty, !manage, !clear {
            args.append(prompt)
        }
        return args
    }
}
