import Foundation

/// Time left on an approval, as `m:ss`. Unanswered at zero means denied.
public enum Countdown {
    public static func text(deadline: Int64, now: Int64) -> String {
        let seconds = max(0, (deadline - now) / 1000)
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }

    public static func nowMs() -> Int64 {
        Int64(Date().timeIntervalSince1970 * 1000)
    }
}

/// A start the phone sent. A lost answer isn't a failure: the hub may have started it, so the
/// app looks the request up by its id until the hub says.
public enum StartState: Equatable, Sendable {
    case started(sessionId: String, alreadyRunning: Bool)
    /// No answer, or the hub is still finding out; reconcile by request id.
    case unknown(requestId: String)
    case failed(message: String)
}

public enum StartFlow {
    public static func run(
        hub: Hub, project: String, prompt: String, machine: String?, agent: String?, notify: Bool, requestId: String
    ) async -> StartState {
        do {
            let r = try await hub.start(project: project, prompt: prompt, machine: machine, agent: agent, notify: notify, requestId: requestId)
            return .started(sessionId: r.sessionId, alreadyRunning: r.outcome == "deduplicated")
        } catch HubError.unreachable {
            return .unknown(requestId: requestId)
        } catch let e as HubError where ["outcome_unknown", "reconciling"].contains(e.code ?? "") {
            return .unknown(requestId: requestId)
        } catch {
            return .failed(message: error.localizedDescription)
        }
    }

    /// One look at the hub's record of the request.
    public static func reconcile(hub: Hub, requestId: String) async -> StartState {
        do {
            let op = try await hub.operation(requestId)
            switch op.state {
            case "done":
                return op.sessionId.map { .started(sessionId: $0, alreadyRunning: false) } ?? .failed(message: "The hub has no session for it.")
            case "failed":
                return .failed(message: op.errorMessage ?? "The start failed.")
            default:
                return .unknown(requestId: requestId)
            }
        } catch let e as HubError where e.code == "unknown_operation" {
            // The request never reached the hub, so retrying with the same id is safe.
            return .failed(message: "The start never reached the hub. Try again.")
        } catch {
            return .unknown(requestId: requestId)
        }
    }
}

extension Data {
    public init?(base64URL: String) {
        var s = base64URL.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        s += String(repeating: "=", count: (4 - s.count % 4) % 4)
        self.init(base64Encoded: s)
    }
}
