import Foundation

/// Posts the core's telemetry queue when consent is on.
/// The endpoint is a stub (`example.invalid`) until a real collector exists.
/// A kill switch is a UserDefaults flag plus an optional remote body of "1".
enum TelemetryClient {
    static let endpoint = URL(string: "https://example.invalid/v1/events")!
    static let killURL = URL(string: "https://example.invalid/v1/kill")!

    static func installCrashHook() {
        NSSetUncaughtExceptionHandler { exception in
            UserDefaults.standard.set(true, forKey: "pendingCrash")
            UserDefaults.standard.set(exception.name.rawValue, forKey: "pendingCrashName")
        }
    }

    static func flushPendingCrash(handle: OpaquePointer, consent: Bool) {
        guard consent, UserDefaults.standard.bool(forKey: "pendingCrash") else { return }
        tank_telemetry_push(handle, 2, 0)
        UserDefaults.standard.set(false, forKey: "pendingCrash")
        flush(handle: handle, consent: true)
    }

    static func flush(handle: OpaquePointer, consent: Bool) {
        guard consent else { return }
        var recs = Array(repeating: TelRec(kind: 0, arg0: 0), count: 32)
        let n = recs.withUnsafeMutableBufferPointer { buf in
            tank_telemetry_drain(handle, buf.baseAddress, UInt32(buf.count))
        }
        guard n > 0 else { return }
        let anon = anonymousID()
        var events = "["
        for i in 0..<Int(n) {
            if i > 0 { events += "," }
            events += "{\"kind\":\(recs[i].kind),\"arg0\":\(recs[i].arg0)}"
        }
        events += "]"
        let body = "{\"anon\":\"\(anon)\",\"events\":\(events)}"
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = Data(body.utf8)
        URLSession.shared.dataTask(with: request).resume()
    }

    /// Remote body "1" is the kill switch. The callback always runs on the main
/// queue; the caller applies it to the tank so the core stays single-threaded.
    static func refreshKillSwitch(apply: @escaping (Bool) -> Void) {
        if UserDefaults.standard.bool(forKey: "telemetryKill") {
            DispatchQueue.main.async { apply(true) }
        }
        URLSession.shared.dataTask(with: killURL) { data, _, _ in
            guard let data, let text = String(data: data, encoding: .utf8) else { return }
            let kill = text.trimmingCharacters(in: .whitespacesAndNewlines) == "1"
            UserDefaults.standard.set(kill, forKey: "telemetryKill")
            DispatchQueue.main.async { apply(kill) }
        }.resume()
    }

    private static func anonymousID() -> String {
        let key = "telemetryAnon"
        if let existing = UserDefaults.standard.string(forKey: key) {
            return existing
        }
        let fresh = UUID().uuidString
        UserDefaults.standard.set(fresh, forKey: key)
        return fresh
    }
}
