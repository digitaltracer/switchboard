import Foundation
import Testing
@testable import SwitchboardKit

private func items(_ json: String) throws -> [Conversation.Item] {
    try JSONDecoder().decode(Schemas.Transcript.self, from: Data(#"{"start":0,"end":1,"items":\#(json)}"#.utf8)).items
}

@Test func stepsBetweenMessagesFoldIntoWorkThatSaysWhatCameOfIt() throws {
    let list = try items(#"""
    [{"id":"u1","kind":"user","text":"add retries, then open a PR","done":false,"failed":false,"at":"2026-10-28T13:12:00Z"},
     {"id":"t1","kind":"tool","tool":"Read","text":"Read /r/slack.py","done":true,"failed":false,"at":"2026-10-28T13:12:05Z"},
     {"id":"t2","kind":"tool","tool":"Edit","text":"Edited /r/slack.py","done":true,"failed":false,"added":40,"removed":6,"at":"2026-10-28T13:14:00Z"},
     {"id":"t3","kind":"tool","tool":"Edit","text":"Edited /r/slack.py","done":true,"failed":false,"added":8,"at":"2026-10-28T13:15:00Z"},
     {"id":"t4","kind":"tool","tool":"Bash","text":"Ran pytest -q","done":true,"failed":false,"at":"2026-10-28T13:16:00Z"},
     {"id":"t5","kind":"tool","tool":"Bash","text":"Ran gh pr create","output":"https://github.com/acme/lib/pull/49","done":true,"failed":false,"at":"2026-10-28T13:18:05Z"},
     {"id":"a1","kind":"assistant","text":"PR #49 is open.","done":false,"failed":false,"at":"2026-10-28T13:18:10Z"},
     {"id":"u2","kind":"user","text":"push both","done":false,"failed":false},
     {"id":"t6","kind":"tool","tool":"Bash","text":"Ran git push","done":true,"failed":false},
     {"id":"a2","kind":"assistant","text":"Pushed.","done":false,"failed":false}]
    """#)
    let entries = Turns.entries(list)
    #expect(entries.map(\.id) == ["u1", "work:t1", "a1", "u2", "work:t6", "a2"])
    guard case .work(let w) = entries[1] else { Issue.record("expected work"); return }
    #expect(w.seconds == 360)
    #expect(w.tags.map(\.text) == ["1 file", "Tests passed", "PR #49"])
    #expect(w.tags[0].added == 48 && w.tags[0].removed == 6)

    let latest = Turns.latest(list)
    #expect(latest.reply?.text == "Pushed.")
    #expect(latest.tags.map(\.text) == ["Pushed"])
}

@Test func failuresAndPlainCommandsGetTheirOwnTags() throws {
    let list = try items(#"""
    [{"id":"t1","kind":"tool","tool":"Bash","text":"Ran cargo test","done":true,"failed":true},
     {"id":"t2","kind":"tool","tool":"Bash","text":"Ran ls","done":true,"failed":true}]
    """#)
    #expect(Turns.tags(list).map(\.text) == ["Tests failed", "1 failed"])
    let plain = try items(#"[{"id":"t1","kind":"tool","tool":"Bash","text":"Ran ls","done":true,"failed":false}]"#)
    #expect(Turns.tags(plain).map(\.text) == ["1 command"])
}

@Test func skillsFilterByNameFirstThenDescription() throws {
    let skills = try JSONDecoder().decode(Schemas.Skills.self, from: Data(#"""
    {"skills":[{"name":"/compact","description":"Summarise","source":"builtin"},
               {"name":"/council","description":"Review a plan","source":"user"},
               {"name":"/ship","description":"Commit, push and open a PR","source":"user"},
               {"name":"/commit","description":"Atomic commits","source":"user"}]}
    """#.utf8)).skills
    #expect(SkillSearch.filter(skills, typed: "/co").map(\.name) == ["/compact", "/council", "/commit", "/ship"])
    #expect(SkillSearch.filter(skills, typed: "/com").map(\.name) == ["/compact", "/commit", "/ship"])
    #expect(SkillSearch.isTyping("/co") && !SkillSearch.isTyping("/commit fix it") && !SkillSearch.isTyping("hi"))
}

@Test func aReplyWithMoreWorkAfterItIsProgressAndTheTurnsLastReplyIsTheAnswer() throws {
    let list = try items(#"""
    [{"id":"u1","kind":"user","text":"review it","done":false,"failed":false},
     {"id":"t1","kind":"tool","tool":"Read","text":"Read /r/a.py","done":true,"failed":false},
     {"id":"a1","kind":"assistant","text":"Still reviewing.","done":false,"failed":false},
     {"id":"t2","kind":"tool","tool":"Bash","text":"Ran pytest","done":true,"failed":false},
     {"id":"a2","kind":"assistant","text":"Tests pass.","done":false,"failed":false},
     {"id":"a3","kind":"assistant","text":"Round 2: no defects.","done":false,"failed":false},
     {"id":"u2","kind":"user","text":"push","done":false,"failed":false},
     {"id":"a4","kind":"assistant","text":"Pushing.","done":false,"failed":false},
     {"id":"t3","kind":"tool","tool":"Bash","text":"Ran git push","done":false,"failed":false}]
    """#)
    let kinds = Turns.entries(list).map { e -> String in
        switch e {
        case .user: "user"
        case .reply: "reply"
        case .progress: "progress"
        case .work: "work"
        case .note: "note"
        }
    }
    #expect(kinds == ["user", "work", "progress", "work", "progress", "reply", "user", "progress", "work"])
}
