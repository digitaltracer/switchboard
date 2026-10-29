import Foundation

/// Whose conversation: a Switchboard session's, or an agent started in Herdr.
public enum ConversationSource: Hashable, Sendable {
    case session(String)
    case agent(machine: String, name: String)
}

/// Following a conversation while it's on screen: the transcript as a long-poll the hub holds
/// until there's more, and the agent's screen once a second while it's shown.
public enum Follow {
    /// How long the hub is asked to hold a transcript request.
    public static let waitMs: Int64 = 20_000
    /// The least time from one request's start to the next: a screen tick, and the pace of a hub
    /// too old to wait.
    public static let every: Duration = .seconds(1)
    /// After a failed request.
    public static let retry: Duration = .seconds(3)

    /// A page of the conversation. With `wait`, one that continues from `after` waits for more;
    /// only following asks for that, since a refresh someone is waiting on mustn't hang.
    public static func page(_ hub: Hub, _ source: ConversationSource, after: Int64?, before: Int64?, wait: Bool = false) async throws -> Schemas.Transcript {
        let wait = wait && after != nil && before == nil ? waitMs : nil
        return switch source {
        case .session(let id): try await hub.transcript(id, after: after, before: before, waitMs: wait)
        case .agent(let machine, let name): try await hub.agentTranscript(machine: machine, agent: name, after: after, before: before, waitMs: wait)
        }
    }

    /// Only the screen: reading further back would scroll the agent's terminal on its machine.
    public static func screen(_ hub: Hub, _ source: ConversationSource) async throws -> Schemas.Output {
        switch source {
        case .session(let id): try await hub.output(id, visible: true, ansi: true)
        case .agent(let machine, let name): try await hub.agentOutput(machine: machine, agent: name, visible: true, ansi: true)
        }
    }

    /// Runs `round` until cancelled or `going` turns false, one at a time, each starting at least
    /// `every` after the last began, or `retry` after one that failed. `round` returns false on failure.
    @MainActor
    public static func repeating(
        every: Duration = every,
        retry: Duration = retry,
        going: () -> Bool = { true },
        now: () -> ContinuousClock.Instant = { ContinuousClock.now },
        sleep: (Duration) async throws -> Void = { try await Task.sleep(for: $0) },
        _ round: () async -> Bool
    ) async {
        while !Task.isCancelled, going() {
            let start = now()
            let ok = await round()
            let rest = (ok ? every : retry) - (now() - start)
            if rest > .zero {
                do { try await sleep(rest) } catch { return }
            }
        }
    }
}
