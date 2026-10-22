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
