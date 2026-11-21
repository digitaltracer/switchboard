import Foundation
import Testing

@testable import SwitchboardKit

@Test func aPullRequestLinkInTheReplyIsFound() {
    let reply = "Pushed the branch and opened [Retry failed imports with backoff](https://github.com/acme/kraft/pull/482) as a draft."
    let pr = PullRequest.find(in: reply)
    #expect(pr?.number == 482)
    #expect(pr?.repo == "acme/kraft")
    #expect(pr?.title == "Retry failed imports with backoff")
    #expect(pr?.url.absoluteString == "https://github.com/acme/kraft/pull/482")
    #expect(PullRequest.find(in: "See https://gitlab.com/g/sub/proj/-/merge_requests/7 now")?.number == 7)
    #expect(PullRequest.find(in: "https://github.com/a/b/pull/1 then https://github.com/a/b/pull/2")?.number == 2)
    #expect(PullRequest.find(in: "https://github.com/a/b/issues/3") == nil)
    #expect(PullRequest.find(in: "nothing here") == nil)
}

@Test func aShipSkillWithoutAPullRequestSaysSo() {
    let pr = ShipButton(skill: "/pr", label: "Raise PR")
    let commit = ShipButton(skill: "/commit", label: "Commit only")
    #expect(ShipOutcome.of(sent: pr, turnEnded: false, reply: "working") == .running)
    #expect(ShipOutcome.of(sent: pr, turnEnded: true, reply: "The push was denied.") == .noPullRequest)
    #expect(ShipOutcome.of(sent: commit, turnEnded: true, reply: "Committed.") == .done)
    if case .opened(let p) = ShipOutcome.of(sent: pr, turnEnded: true, reply: "https://github.com/a/b/pull/9") { #expect(p.number == 9) } else { Issue.record("expected a PR") }
    #expect(ShipButton(skill: "/create-pr", label: "x").opensPR)
    #expect(!ShipButton(skill: "$fix-ci", label: "x").opensPR)
}

@Test func buttonsComeFromTheProjectThenAllThenSuggestions() {
    let v = Schemas.ShipButtons(all: [.init(skill: "pr", label: "Raise PR", afterPr: false)],
                                projects: [.init(project: "kraft", buttons: [.init(skill: "ship", label: "Ship", afterPr: false)])])
    #expect(ShipButtons.effective(v, project: "kraft").map(\.skill) == ["ship"])
    #expect(ShipButtons.effective(v, project: "other").map(\.skill) == ["pr"])
    #expect(ShipButtons.suggested(from: ["fix-ci", "pr", "review"]).map(\.skill) == ["pr", "fix-ci"])
    #expect(ShipButtons.label(for: "plugin:release_note") == "Release note")
    #expect(ShipButton(skill: "/pr", label: "Raise PR").message(" Draft, base main ") == "/pr Draft, base main")
}
