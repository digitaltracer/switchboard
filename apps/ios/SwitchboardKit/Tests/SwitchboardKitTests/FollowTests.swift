import Foundation
import Testing

@testable import SwitchboardKit

private let page = #"{"items":[],"start":10,"end":10}"#
private let screen = #"{"text":"> _","truncated":false}"#

private func following() -> (FakeHub, Hub) {
    let fake = FakeHub { op, _, _ in
        tokenAnswers(op) ?? (200, op.hasSuffix("transcript") ? page : screen)
    }
    return (fake, Hub(pairing: pairing, keys: SoftwareKeys(), build: 1, transport: fake))
}

private func query(_ fake: FakeHub, _ op: String) -> [String] {
    fake.requests.filter { $0.op == op }.map { $0.request.path?.split(separator: "?").dropFirst().joined() ?? "" }
}

@Test func onlyFollowingAsksTheHubToWaitAndOnlyAfterTheFirstLoad() async throws {
    let (fake, hub) = following()
    for source in [ConversationSource.session("s1"), .agent(machine: "mac", name: "mono1/claude")] {
        _ = try await Follow.page(hub, source, after: nil, before: nil, wait: true)
        _ = try await Follow.page(hub, source, after: 10, before: nil, wait: true)
        _ = try await Follow.page(hub, source, after: nil, before: 10, wait: true)
        // A refresh someone is waiting on, such as pull to refresh.
        _ = try await Follow.page(hub, source, after: 10, before: nil)
    }
    let asked = query(fake, "get_transcript") + query(fake, "get_agent_transcript")
    #expect(asked.count == 8)
    #expect(asked.filter { $0.contains("wait_ms=20000") }.count == 2, "\(asked)")
    #expect(asked.filter { $0.contains("wait_ms") && !$0.contains("after=10") }.isEmpty, "only continuing reads wait")
}

@Test func theScreenIsReadVisibleOnly() async throws {
    let (fake, hub) = following()
    _ = try await Follow.screen(hub, .session("s1"))
    _ = try await Follow.screen(hub, .agent(machine: "mac", name: "mono1/claude"))
    let asked = query(fake, "get_output") + query(fake, "get_agent_output")
    #expect(asked.count == 2)
    #expect(asked.allSatisfy { $0.contains("visible=true") }, "\(asked)")
}

/// Virtual time: sleeping and a round's own work move it on at once.
@MainActor
private final class Clock {
    let base = ContinuousClock.now
    var elapsed: Duration = .zero
    var starts: [Duration] = []
    func now() -> ContinuousClock.Instant { base + elapsed }
    func sleep(_ d: Duration) { elapsed += d }
}

@MainActor
private func run(_ clock: Clock, rounds: Int, going: @escaping () -> Bool = { true }, _ round: @escaping (Int) -> (Duration, Bool)) async {
    var n = 0
    await Follow.repeating(going: { n < rounds && going() }, now: clock.now, sleep: { clock.sleep($0) }) {
        clock.starts.append(clock.elapsed)
        let (took, ok) = round(n)
        clock.elapsed += took
        n += 1
        return ok
    }
}

@Test @MainActor func aHubThatDoesntWaitIsAskedNoMoreThanOnceASecond() async {
    let clock = Clock()
    await run(clock, rounds: 4) { _ in (.milliseconds(30), true) }
    #expect(clock.starts == [.zero, .seconds(1), .seconds(2), .seconds(3)])
}

@Test @MainActor func aHubThatWaitsIsAskedAgainAsSoonAsItAnswers() async {
    let clock = Clock()
    await run(clock, rounds: 3) { n in (n == 1 ? .milliseconds(400) : .seconds(20), true) }
    #expect(clock.starts == [.zero, .seconds(20), .milliseconds(21_000)])
}

@Test @MainActor func aFailedRequestBacksOff() async {
    let clock = Clock()
    await run(clock, rounds: 3) { n in (.milliseconds(100), n != 0) }
    #expect(clock.starts == [.zero, .seconds(3), .seconds(4)])
}

@Test @MainActor func nothingIsAskedOnceTheAppLeavesTheForeground() async {
    let clock = Clock()
    var foreground = true
    await run(clock, rounds: 10, going: { foreground }) { n in
        if n == 1 { foreground = false }
        return (.seconds(20), true)
    }
    #expect(clock.starts.count == 2)
}

/// Counts screen reads and how many ran at once.
@MainActor
private final class Reads {
    var count = 0
    var inFlight = 0
    var most = 0
    func read() async -> Bool {
        count += 1
        inFlight += 1
        most = max(most, inFlight)
        try? await Task.sleep(for: .milliseconds(20))
        inFlight -= 1
        return true
    }
}

@Test @MainActor func aHiddenScreenIsNotReadUntilItIsShown() async throws {
    let (bell, reads) = (Bell(), Reads())
    var shown = false
    let loop = Task { @MainActor in
        await Follow.screens(bell: bell, shown: { shown }) { await reads.read() }
    }
    try await Task.sleep(for: .milliseconds(1500))
    #expect(reads.count == 0, "nothing wakes it while the conversation is shown")
    shown = true
    bell.ring()
    try await Task.sleep(for: .milliseconds(200))
    #expect(reads.count == 1, "opening the Screen tab reads it at once")
    loop.cancel()
    await loop.value
}

@Test @MainActor func openingTheScreenBringsTheNextReadForwardWithoutAnotherAlongside() async throws {
    let (bell, reads) = (Bell(), Reads())
    let loop = Task { @MainActor in
        await Follow.screens(bell: bell, shown: { true }, every: .seconds(30)) { await reads.read() }
    }
    try await Task.sleep(for: .milliseconds(5))
    bell.ring()
    bell.ring()
    try await Task.sleep(for: .milliseconds(200))
    #expect(reads.count == 2, "a ring during a read starts one more after it, not one beside it")
    #expect(reads.most == 1)
    loop.cancel()
    await loop.value
}
