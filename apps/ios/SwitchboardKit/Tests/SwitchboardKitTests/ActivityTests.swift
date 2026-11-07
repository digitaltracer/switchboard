import Foundation
import Testing
@testable import SwitchboardKit

private func decode<T: Decodable>(_ json: String) throws -> T {
    try JSONDecoder().decode(T.self, from: Data(json.utf8))
}

private func session(_ id: String, machine: String = "mac") throws -> Schemas.Session {
    try decode("""
    {"id":"\(id)","label":"kraft-\(id)","project_id":"kraft","machine_id":"\(machine)","machine_status":"ready",
     "agent":"claude","lifecycle":"running","agent_status":"working","hook_failed":false,"revision":1,
     "created_at":0,"updated_at":0,"muted":false,"notifies":true,"pending_approvals":[]}
    """)
}

private func live(_ name: String, machine: String = "mac") throws -> Schemas.AgentView {
    try decode(#"{"machine_id":"\#(machine)","name":"\#(name)","agent_status":"working"}"#)
}

private let withActivity = """
{"agents": [{"machine_id":"mac","name":"crm/claude","agent_status":"working"},
            {"machine_id":"vps","name":"api/codex","agent_status":"working"}],
 "ended": [],
 "activity": [{"machine_id":"mac","name":"crm/claude","line":"Ran cargo test","at":"2026-11-07T09:00:00Z"},
              {"machine_id":"mac","name":"crm/w1:p2"},
              {"machine_id":"mac","name":"sb-9f3c","session_id":"9f3c","line":"Tests pass."}],
 "activity_machines": ["mac"]}
"""

@Test func tilesReadTheirLineFromActivity() throws {
    let lines = try #require(TileLines(try decode(withActivity)))
    #expect(lines.line(for: .live(try live("crm/claude"))) == .known("Ran cargo test"))
    #expect(lines.line(for: .task(try session("9f3c"))) == .known("Tests pass."))
    #expect(lines.line(for: .live(try live("crm/w1:p2"))) == .known(nil), "no line yet means none, not a read")
    #expect(lines.line(for: .live(try live("api/codex", machine: "vps"))) == .unknown, "an older runner's machine")
    #expect(lines.line(for: .task(try session("0000", machine: "vps"))) == .unknown)
}

@Test func anOlderHubGivesNoActivity() throws {
    let old: Schemas.Agents = try decode(#"{"agents":[],"ended":[]}"#)
    #expect(TileLines(old) == nil)
}

@Test func aTileOutsideActivityReadsItsConversationAtMostEvery30Seconds() {
    var pacer = FallbackPacer()
    let t0 = Date(timeIntervalSince1970: 1000)
    let at = { (id: String, s: TimeInterval) in pacer.due(id, now: t0.addingTimeInterval(s)) }
    #expect(at("a", 0) == true)
    #expect(at("a", 5) == false)
    #expect(at("b", 5) == true, "each tile on its own")
    #expect(at("a", 29) == false)
    #expect(at("a", 30) == true)
}
