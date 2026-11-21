import Foundation
import Testing

@testable import SwitchboardKit

private func agent(_ state: String?, _ facts: [(Int64, String)], reply: String? = nil) -> Schemas.AwayAgentView {
    .init(machineId: "mac", agent: "checkout", sessionId: nil, state: state,
          facts: facts.map { .init(at: $0.0, kind: $0.1, text: $0.1) }, lastReply: reply)
}

@Test func eachAgentGetsOneLineSayingWhatItIsDoingOrDid() {
    let now: Int64 = 10_000_000
    #expect(AwayDigest.line(agent("waiting", [(now - 22 * 60_000, "waiting")], reply: "asks which provider to keep"), now: now)
        == "Waiting 22 min: \"asks which provider to keep\"")
    #expect(AwayDigest.line(agent("idle", [(1, "working"), (2, "finished")], reply: "Added backoff."), now: now) == "Finished: \"Added backoff.\"")
    #expect(AwayDigest.line(agent("error", [(1, "error")]), now: now) == "Stopped with an error")
    #expect(AwayDigest.line(agent("working", [(1, "asked"), (2, "allowed"), (3, "asked"), (4, "allowed")]), now: now)
        == "Still working · you allowed 2 commands meanwhile")
    #expect(AwayDigest.duration(100 * 60) == "1 h 40 m")
    #expect(AwayDigest.duration(30) == "under a minute")
    #expect(AwayDigest.tone(agent("waiting", [])) == "need")
}

@Test func consecutiveTurnsTellAShortStory() {
    let f = { (at: Int64, kind: String) in Schemas.AwayFactView(at: at, kind: kind, text: kind) }
    let story = AwayDigest.timeline([f(1, "started"), f(2, "asked"), f(3, "denied"), f(4, "finished"), f(5, "working"), f(6, "finished"),
                                     f(7, "working"), f(8, "finished")])
    #expect(story.map(\.kind) == ["started", "asked", "denied", "finished"])
    #expect(story.last?.text == "Finished 3 turns")
    #expect(story.last?.at == 8)
    let one = AwayDigest.timeline([f(1, "working"), f(2, "finished")])
    #expect(one.map(\.text) == ["Finished its turn"])
    let still = AwayDigest.timeline([f(1, "finished"), f(2, "asked"), f(3, "working")])
    #expect(still.map(\.kind) == ["finished", "asked", "working"], "a turn still going keeps its row")
    let long = AwayDigest.timeline((0..<9).map { f(Int64($0), $0 % 2 == 0 ? "asked" : "allowed") })
    #expect(long.count == AwayDigest.maxRows)
    #expect(long.last?.at == 8, "the newest are kept")
}
