import Foundation
import Testing
@testable import SwitchboardKit

private func decode<T: Decodable>(_ json: String) throws -> T {
    try JSONDecoder().decode(T.self, from: Data(json.utf8))
}

private func session(_ id: String, project: String, lifecycle: String = "running") throws -> Schemas.Session {
    try decode("""
    {"id":"\(id)","label":"\(project)-\(id)","project_id":"\(project)","machine_id":"mac","machine_status":"ready",
     "agent":"claude","lifecycle":"\(lifecycle)","agent_status":"working","hook_failed":false,"revision":1,
     "created_at":0,"updated_at":0,"muted":false,"notifies":true,"pending_approvals":[]}
    """)
}

@Test func aStartOffersTheProjectsRunningSessionsToo() throws {
    let preview: Schemas.MessagePreview = try decode(
        #"{"outcome":"start","message":"Starts a new session in kraft on mac.","project_id":"kraft","machine_id":"mac","options":[]}"#
    )
    let sessions = [try session("9f3c", project: "kraft"), try session("1a2b", project: "switchboard"),
                    try session("0000", project: "kraft", lifecycle: "stopped")]
    let r = RouteOptions.build(preview: preview, sessions: sessions, context: nil)
    #expect(r.options.map(\.title) == ["New task in kraft", "Continue kraft-9f3c"])
    #expect(r.suggested == "start:kraft")
    #expect(r.options[0].action == .asPreviewed)
    #expect(r.options[1].action == .send(.session(sessions[0])))
}

@Test func aSendOffersANewTaskInTheSameProject() throws {
    let preview: Schemas.MessagePreview = try decode(
        #"{"outcome":"send","message":"Sends to kraft-9f3c.","target":{"kind":"session","session_id":"9f3c","machine_id":"mac","label":"kraft-9f3c"},"options":[]}"#
    )
    let r = RouteOptions.build(preview: preview, sessions: [try session("9f3c", project: "kraft")], context: nil)
    #expect(r.options.map(\.title) == ["Continue kraft-9f3c", "New task in kraft"])
    #expect(r.options[1].action == .start(projectId: "kraft"))
    #expect(r.options[0].verb == "Send to kraft-9f3c")
}

@Test func whenTheHubWouldAskNothingIsSuggestedAndTheContextComesFirst() throws {
    let preview: Schemas.MessagePreview = try decode(
        #"{"outcome":"choose_project","message":"Which project?","options":[{"label":"kraft","project_id":"kraft"},{"label":"niti","project_id":"niti"}]}"#
    )
    let context = Schemas.TargetView.agent(machine: "mac", name: "mono1/claude")
    let r = RouteOptions.build(preview: preview, sessions: [], context: context)
    #expect(r.suggested == nil)
    #expect(r.options.map(\.title) == ["Continue mono1/claude", "New task in kraft", "New task in niti"])
}

@Test func changesDecodeWithoutCountsForNewFiles() throws {
    let c: Schemas.Changes = try decode(#"{"files":[{"path":"a.rs","added":3,"deleted":1},{"path":"new.rs"}],"truncated":false}"#)
    #expect(c.files.map(\.added) == [3, nil])
}

@Test func anAgentWithoutANameShowsWhatItsTerminalSays() throws {
    let agents: Schemas.Agents = try decode(#"""
    {"agents":[{"machine_id":"mac","name":"mono1/claude","agent_status":"idle","title":"Ignored"},
               {"machine_id":"mac","name":"mono1/w1:p2","agent_status":"idle","title":"Fix the flaky test"},
               {"machine_id":"mac","name":"mono1/w1:p3","agent_status":"idle"}]}
    """#)
    #expect(agents.agents.map(\.displayName) == ["mono1/claude", "Fix the flaky test", "mono1/w1:p3"])
}

private func page(_ json: String) throws -> Schemas.Transcript { try decode(json) }

@Test func aConversationGrowsJoinsResultsAndGoesBack() throws {
    var c = Conversation()
    #expect(!c.isLoaded)
    c.replace(with: try page(#"""
    {"start":500,"end":900,"items":[
      {"id":"u1","kind":"user","text":"fix it","done":false,"failed":false},
      {"id":"t1","kind":"tool","text":"Ran cargo test","tool":"Bash","done":false,"failed":false}]}
    """#))
    #expect(c.items.map(\.id) == ["u1", "t1"] && c.hasEarlier)

    // The result comes on the next page and fills in its call.
    c.append(try page(#"""
    {"start":900,"end":1000,"items":[
      {"id":"t1","kind":"tool","output":"ok","done":true,"failed":false},
      {"id":"a1","kind":"assistant","text":"Fixed.","done":false,"failed":false}]}
    """#), askedAfter: 900)
    #expect(c.items.map(\.id) == ["u1", "t1", "a1"])
    #expect(c.items[1].text == "Ran cargo test" && c.items[1].output == "ok" && c.items[1].done)
    #expect(c.end == 1000)

    // An earlier page's call meets a result that arrived before it.
    c.append(try page(#"""
    {"start":1000,"end":1100,"items":[{"id":"t0","kind":"tool","output":"done earlier","done":true,"failed":true}]}
    """#), askedAfter: 1000)
    c.prepend(try page(#"""
    {"start":0,"end":500,"items":[{"id":"t0","kind":"tool","text":"Ran make","tool":"Bash","done":false,"failed":false}]}
    """#))
    #expect(c.items.map(\.id) == ["t0", "u1", "t1", "a1"] && !c.hasEarlier)
    #expect(c.items[0].output == "done earlier" && c.items[0].failed)

    // A page that skipped some replaces what's shown.
    c.append(try page(#"""
    {"start":5000,"end":5100,"items":[{"id":"a9","kind":"assistant","text":"Later.","done":false,"failed":false}]}
    """#), askedAfter: 1100)
    #expect(c.items.map(\.id) == ["a9"] && c.start == 5000)
}
