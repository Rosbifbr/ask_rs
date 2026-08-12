import Foundation

enum ChildEnvironment {
    static func variables() -> [String] {
        var map = dumpLoginEnvironment() ?? [:]
        for (key, value) in ProcessInfo.processInfo.environment {
            map[key] = value
        }
        loadDotEnv(into: &map)

        map["TERM"] = "xterm-256color"
        map["COLORTERM"] = "truecolor"
        map["TERM_PROGRAM"] = "Ask"

        if map["HOME"] == nil {
            map["HOME"] = NSHomeDirectory()
        }
        if map["USER"] == nil {
            map["USER"] = NSUserName()
        }
        if map["LANG"] == nil {
            map["LANG"] = "en_US.UTF-8"
        }

        var path = map["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin"
        for extra in ["/opt/homebrew/bin", "/usr/local/bin"] where !path.split(separator: ":").contains(where: { $0 == extra }) {
            path = extra + ":" + path
        }
        map["PATH"] = path

        return map.keys.sorted().compactMap { key in
            guard let value = map[key] else { return nil }
            return "\(key)=\(value)"
        }
    }

    static func hasAPIKey(in env: [String]) -> Bool {
        env.contains { entry in
            guard let eq = entry.firstIndex(of: "=") else { return false }
            let key = entry[..<eq]
            let value = entry[entry.index(after: eq)...]
            return key.hasSuffix("_API_KEY") && !value.isEmpty
        }
    }

    private static func dumpLoginEnvironment() -> [String: String]? {
        let shell = ProcessInfo.processInfo.environment["SHELL"] ?? "/bin/zsh"
        let name = URL(fileURLWithPath: shell).lastPathComponent

        let proc = Process()
        proc.executableURL = URL(fileURLWithPath: shell)
        if name == "fish" {
            proc.arguments = ["-l", "-c", "set -e fish_greeting; env -0"]
        } else {
            proc.arguments = ["-lc", "env -0"]
        }
        let pipe = Pipe()
        proc.standardOutput = pipe
        proc.standardError = FileHandle.nullDevice

        do {
            try proc.run()
        } catch {
            return nil
        }

        let group = DispatchGroup()
        group.enter()
        DispatchQueue.global(qos: .userInitiated).async {
            proc.waitUntilExit()
            group.leave()
        }
        if group.wait(timeout: .now() + 2) == .timedOut {
            proc.terminate()
            return nil
        }
        guard proc.terminationStatus == 0 else { return nil }

        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        var map: [String: String] = [:]
        for chunk in data.split(separator: 0) {
            guard let raw = String(bytes: chunk, encoding: .utf8) else { continue }
            let line = raw.split(separator: "\n").last.map(String.init) ?? raw
            guard let eq = line.firstIndex(of: "=") else { continue }
            let key = String(line[line.startIndex..<eq])
            guard isEnvKey(key) else { continue }
            map[key] = String(line[line.index(after: eq)...])
        }
        return map.isEmpty ? nil : map
    }

    private static func loadDotEnv(into map: inout [String: String]) {
        let path = (NSHomeDirectory() as NSString).appendingPathComponent(".config/ask.env")
        guard let text = try? String(contentsOfFile: path, encoding: .utf8) else { return }
        for raw in text.split(whereSeparator: \.isNewline) {
            let line = raw.trimmingCharacters(in: .whitespaces)
            if line.isEmpty || line.hasPrefix("#") { continue }
            let body = line.hasPrefix("export ") ? String(line.dropFirst(7)) : String(line)
            guard let eq = body.firstIndex(of: "=") else { continue }
            let key = String(body[body.startIndex..<eq]).trimmingCharacters(in: .whitespaces)
            guard isEnvKey(key) else { continue }
            var value = String(body[body.index(after: eq)...]).trimmingCharacters(in: .whitespaces)
            if (value.hasPrefix("\"") && value.hasSuffix("\"")) || (value.hasPrefix("'") && value.hasSuffix("'")) {
                value = String(value.dropFirst().dropLast())
            }
            map[key] = value
        }
    }

    private static func isEnvKey(_ key: String) -> Bool {
        guard let first = key.first else { return false }
        if first != "_" && !first.isLetter { return false }
        return key.allSatisfy { $0 == "_" || $0.isLetter || $0.isNumber }
    }
}
