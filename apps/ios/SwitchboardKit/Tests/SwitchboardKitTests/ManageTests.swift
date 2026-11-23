import Foundation
import Testing
@testable import SwitchboardKit

private func decode<T: Decodable>(_ json: String) throws -> T {
    try JSONDecoder().decode(T.self, from: Data(json.utf8))
}

/// A session as a hub that knows about renames lists it; `extra` is more JSON fields.
private func task(_ id: String, herdr: String, project: String = "kraft", lifecycle: String = "running", extra: String = "") throws -> Schemas.Session {
    try decode("""
    {"id":"\(id)","label":"\(project)-\(id)","project_id":"\(project)","machine_id":"mac","machine_status":"ready",
     "agent":"claude","lifecycle":"\(lifecycle)","agent_status":"idle","hook_failed":false,"revision":1,
     "created_at":0,"updated_at":0,"muted":false,"notifies":true,"pending_approvals":[],"herdr_session":"\(herdr)"\(extra)}
    """)
}

private func agent(_ name: String, extra: String = "") throws -> Schemas.AgentView {
    let session = name.split(separator: "/").first!
    return try decode(#"{"machine_id":"mac","name":"\#(name)","herdr_session":"\#(session)","agent_status":"idle"\#(extra)}"#)
}

private func spaces(_ sessions: [Schemas.Session] = [], _ agents: [Schemas.AgentView] = [], ended: [Schemas.EndedAgent] = []) -> [AgentSpace] {
    AgentSpace.group(sessions: sessions, agents: agents, ended: ended, since: nil)
}

@Test func aHerdrAppSpaceCanBeRenamedAndTakeAnAgentWhenItsAgentsAgreeOnItsId() throws {
    let all = spaces(
        [try task("9f3c", herdr: "hc", extra: #","space":"atlas","space_id":"sp1","can_rename":true"#)],
        [try agent("hc/search", extra: #","space":"atlas","space_id":"sp1","can_rename":true"#),
         try agent("hc/scratch", extra: #","space":"core","can_rename":false"#)]
    )
    let atlas = try #require(all.first { $0.title == "atlas" })
    #expect(atlas.spaceId == "sp1")
    #expect(atlas.rename == .yes && atlas.newAgent == .yes)
    #expect(atlas.project == "kraft", "its one session's project is offered first")
    // Agents outside every space have no space to rename or start into, and nothing to say.
    let outside = try #require(all.first { $0.key.space == nil })
    #expect(outside.spaceId == nil)
    #expect(outside.rename == .hidden && outside.newAgent == .hidden)
}

@Test func twoSpacesWithOneNameAreLeftToTheMac() throws {
    let all = spaces(
        [try task("9f3c", herdr: "hc", extra: #","space":"kraft","space_id":"sp1","can_rename":true"#),
         try task("0a1b", herdr: "hc", project: "atlas", extra: #","space":"kraft","space_id":"sp2","can_rename":true"#)]
    )
    #expect(all.count == 1, "the phone shows them as one")
    #expect(all[0].spaceId == nil)
    #expect(all[0].rename == .no("Several spaces share this name. Rename one at the Mac."))
    #expect(all[0].newAgent == .no("Several spaces share this name. Start it at the Mac."))
    #expect(all[0].project == nil, "two projects: none is picked for you")
}

@Test func aHerdrTerminalSpaceIsRenamedOnlyWhereItsWorkspaceNameShows() throws {
    let all = spaces(
        [try task("9f3c", herdr: "kraft-9f3c", extra: #","space":"kraft","space_id":"w1","can_rename":true"#)],
        [try agent("main/api", extra: #","space":"billing","space_id":"w1","can_rename":true"#),
         try agent("main/web", extra: #","space":"atlas","space_id":"w2","can_rename":true"#)]
    )
    let billing = try #require(all.first { $0.title == "main › billing" })
    #expect(billing.rename == .yes && billing.spaceId == "w1")
    #expect(billing.newAgent == .no("New agents here start at the Mac."))
    // One workspace: the phone shows the Herdr session's name, which Herdr can't change.
    let own = try #require(all.first { $0.title == "kraft-9f3c" })
    #expect(own.rename == .no("This Herdr session's name is fixed. Rename a workspace at the Mac."))
    #expect(own.newAgent == .no("New agents here start at the Mac."))
}

@Test func anOlderHubOrAnEndedSpaceOffersNothing() throws {
    let old = spaces([], [try agent("hc/search", extra: #","space":"atlas""#)])
    #expect(old[0].rename == .hidden && old[0].newAgent == .hidden)
    let ended: Schemas.EndedAgent = try decode(#"{"machine_id":"mac","name":"hc/old","herdr_session":"hc","started_at":0,"ended_at":5,"space":"gone"}"#)
    let gone = spaces(ended: [ended])
    #expect(gone[0].rename == .hidden && gone[0].newAgent == .hidden)
}

@Test func anAgentSaysWhetherItCanBeRenamed() throws {
    let session = HerdrSession.Agent.task(try task("9f3c", herdr: "hc", extra: #","can_rename":true"#))
    #expect(session.rename == .yes && !session.isHerdrAgent)
    #expect(HerdrSession.Agent.task(try task("9f3c", herdr: "hc")).rename == .hidden, "an older hub")
    #expect(HerdrSession.Agent.task(try task("9f3c", herdr: "hc", lifecycle: "deleted", extra: #","can_rename":true"#)).rename == .hidden)

    let own = HerdrSession.Agent.live(try agent("main/api", extra: #","can_rename":true"#))
    #expect(own.rename == .yes && own.isHerdrAgent)
    let fixed = HerdrSession.Agent.live(try agent("hc/review", extra: #","can_rename":false"#))
    #expect(fixed.rename == .no("Rename it at the Mac.") && !fixed.isHerdrAgent)
    #expect(HerdrSession.Agent.live(try agent("hc/review")).rename == .hidden, "an older hub")
    let ended: Schemas.EndedAgent = try decode(#"{"machine_id":"mac","name":"hc/old","herdr_session":"hc","started_at":0,"ended_at":5}"#)
    #expect(HerdrSession.Agent.ended(ended).rename == .hidden)
}

@Test func namesAreCheckedBeforeTheyAreSent() {
    #expect(Names.problem("login fix") == nil)
    #expect(Names.problem("  ") == "Give it a name.")
    #expect(Names.problem(String(repeating: "x", count: 40)) == nil)
    #expect(Names.problem(String(repeating: "x", count: 41)) == "Keep it to 40 characters.")
    #expect(Names.problem("a/b") == "A name can't contain a slash or a line break.")
    #expect(Names.problem("two\nlines") == "A name can't contain a slash or a line break.")
    // Herdr's own rule for an agent in a Herdr terminal.
    #expect(Names.problem("reviewer-2_a", herdrAgent: true) == nil)
    for bad in ["Reviewer", "2fast", "my agent", "é", String(repeating: "a", count: 33)] {
        #expect(Names.problem(bad, herdrAgent: true) == Names.herdrAgentRule, "\(bad)")
    }
    #expect(Names.problem("", herdrAgent: true) == "Give it a name.")
}

@Test func aFailedRenameIsSaidInWords() {
    let hub = { (code: String) in HubError.hub(code: code, message: "the hub's own words", operationId: nil, sessionId: nil) }
    #expect(Names.failure(hub("name_taken")) == "That name is taken.")
    #expect(Names.failure(hub("machine_offline")) == "That machine is offline.")
    #expect(Names.failure(hub("not_found")) == "It's no longer there.")
    #expect(Names.failure(hub("unknown_session")) == "It's no longer there.")
    #expect(Names.failure(hub("busy")) == "The layout changed. Try again.")
    #expect(Names.failure(hub("runner_too_old")) == "Update Switchboard on that machine first.")
    #expect(Names.failure(hub("outcome_unknown")) == "No answer. Check the name in a moment.")
    #expect(Names.failure(hub("bad_request")) == "the hub's own words")
    #expect(Names.failure(HubError.unreachable("timed out")) == "Can't reach the hub: timed out")
}

@Test func renamesAndAStartIntoASpaceSendWhatTheHubExpects() async throws {
    let fake = FakeHub { op, _, _ in
        if let t = tokenAnswers(op) { return t }
        switch op {
        case "rename_agent": return (409, #"{"error":{"code":"name_taken","message":"another agent is already called reviewer"}}"#)
        case "start_session":
            return (200, #"{"session_id":"s1","label":"kraft-s1","project_id":"kraft","machine_id":"mac","agent":"claude","muted":false,"outcome":"started","operation_id":"r3"}"#)
        default: return (200, #"{"name":"billing"}"#)
        }
    }
    let hub = Hub(pairing: pairing, keys: SoftwareKeys(), build: 1, transport: fake)
    let body = { (op: String) in
        try JSONSerialization.jsonObject(with: fake.requests.first { $0.op == op }!.body) as! [String: Any]
    }

    #expect(try await hub.renameSession("s1", name: "billing") == "billing")
    #expect(fake.requests.first { $0.op == "rename_session" }?.request.path == "/app/v1/sessions/s1/rename")
    #expect(try body("rename_session")["name"] as? String == "billing")

    #expect(try await hub.renameSpace(machine: "mac", herdrSession: "hc", spaceId: "sp1", name: "billing", requestId: "r1") == "billing")
    let space = try body("rename_space")
    #expect(space["space_id"] as? String == "sp1" && space["herdr_session"] as? String == "hc")
    #expect(space["machine_id"] as? String == "mac" && space["request_id"] as? String == "r1")

    do {
        _ = try await hub.renameAgent(machine: "mac", agent: "hc/2", name: "reviewer", requestId: "r2")
        Issue.record("expected an error")
    } catch {
        #expect(Names.failure(error) == "That name is taken.")
    }
    #expect(try body("rename_agent")["agent"] as? String == "hc/2")

    let state = await StartFlow.run(hub: hub, project: "kraft", prompt: "fix it", machine: "mac", agent: nil, notify: true,
                                    requestId: "r3", backend: "hc", spaceId: "sp1")
    #expect(state == .started(sessionId: "s1", alreadyRunning: false))
    let start = try body("start_session")
    #expect(start["space_id"] as? String == "sp1" && start["backend"] as? String == "hc")
}
