import Foundation
import Testing
@testable import SwitchboardKit

private func decode<T: Decodable>(_ json: String) throws -> T {
    try JSONDecoder().decode(T.self, from: Data(json.utf8))
}

private func task(_ id: String, herdr: String, lifecycle: String = "running", updated: Int64 = 0) throws -> Schemas.Session {
    try decode("""
    {"id":"\(id)","label":"kraft-\(id)","project_id":"kraft","machine_id":"mac","machine_status":"ready",
     "agent":"claude","lifecycle":"\(lifecycle)","agent_status":"idle","hook_failed":false,"revision":1,
     "created_at":0,"updated_at":\(updated),"muted":false,"notifies":true,"pending_approvals":[],"herdr_session":"\(herdr)"}
    """)
}

private func agent(_ name: String, status: String = "idle", title: String? = nil, space: String? = nil) throws -> Schemas.AgentView {
    let session = name.split(separator: "/").first!
    let t = title.map { #","title":"\#($0)""# } ?? ""
    let s = space.map { #","space":"\#($0)""# } ?? ""
    return try decode(#"{"machine_id":"mac","name":"\#(name)","herdr_session":"\#(session)","agent_status":"\#(status)"\#(t)\#(s)}"#)
}

private func ended(_ name: String, at: Int64, space: String? = nil) throws -> Schemas.EndedAgent {
    let session = name.split(separator: "/").first!
    let s = space.map { #","space":"\#($0)""# } ?? ""
    return try decode(#"{"machine_id":"mac","name":"\#(name)","herdr_session":"\#(session)","started_at":0,"ended_at":\#(at)\#(s)}"#)
}

@Test func theHerdrAppIsOneSessionWithItsSpacesKeptApart() throws {
    let groups = HerdrSession.group(
        sessions: [],
        agents: [
            try agent("hc/orchestrator", status: "working", space: "switchboard"),
            try agent("hc/orchestrator-t9", space: "udp-backend"),
            try agent("hc/reviewer", space: "udp-backend"),
            try agent("hc/scratch", space: "core"),
            try agent("crm/orchestrator", space: "Imports"),
            try agent("crm/w1:p2", title: "Fix the import", space: "1"),
        ],
        ended: [try ended("hc/old", at: 300, space: "switchboard"), try ended("hc/older", at: 200)],
        since: nil
    )
    #expect(groups.map(\.title) == ["crm", "Herdr App"])
    let app = groups[1]
    #expect(app.isHerdrApp && app.key.name == "hc")
    #expect(app.spaces.map(\.name) == ["switchboard", "udp-backend", nil])
    #expect(app.spaces[0].agents.map(\.nameInSpace) == ["orchestrator", "old"])
    #expect(app.spaces[1].live.map(\.nameInSpace) == ["orchestrator-t9", "reviewer"])
    // Outside every space, and one that ended before spaces were recorded.
    #expect(app.spaces[2].agents.map(\.nameInSpace) == ["scratch", "older"])
    #expect(app.live[0].shortName == "switchboard › orchestrator", "elsewhere it still says where")
    // A Herdr session is split the same way; its numbered default workspace is no name.
    #expect(groups[0].spaces.map(\.name) == ["Imports", nil])
    #expect(groups[0].spaces[1].live.map(\.nameInSpace) == ["Fix the import"])
}

@Test func agentsGroupBySessionWithEndedOnesLast() throws {
    let groups = HerdrSession.group(
        sessions: [try task("9f3c", herdr: "kraft-9f3c"), try task("0000", herdr: "kraft-0000", lifecycle: "exited", updated: 500)],
        agents: [try agent("crm/orchestrator", status: "working"), try agent("crm/w1:p2", title: "Fix the import")],
        ended: [try ended("crm/reviewer", at: 300), try ended("crm/old", at: 50), try ended("mono1/w1:p1", at: 400)],
        since: 100
    )
    #expect(groups.map(\.key.name) == ["crm", "kraft-9f3c", "kraft-0000", "mono1"])
    let crm = groups[0]
    #expect(crm.agents.map(\.shortName) == ["orchestrator", "Fix the import", "reviewer"], "ended before `since` is left out")
    #expect(crm.live[1].title == "Fix the import")
    #expect(crm.ended.map(\.agentStatus) == ["ended"])
    #expect(groups[2].isEnded && groups[3].isEnded)
    #expect(groups[1].live.first?.shortName == "kraft-9f3c")
}

@Test func aMachineKeepsOnlyItsOwnAndNilSinceKeepsEveryEnded() throws {
    let groups = HerdrSession.group(
        sessions: [try task("0000", herdr: "kraft-0000", lifecycle: "stopped", updated: 1)],
        agents: [], ended: [], machine: "vps", since: nil
    )
    #expect(groups.isEmpty)
    let all = HerdrSession.group(
        sessions: [try task("0000", herdr: "kraft-0000", lifecycle: "stopped", updated: 1)],
        agents: [], ended: [], since: nil
    )
    #expect(all.count == 1 && all[0].isEnded)
}
