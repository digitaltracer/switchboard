import XCTest

/// Walks the installed, paired app and keeps a screenshot of each screen, for design review.
/// It runs against the owner's own hub, sends nothing, and answers only dialogs of agents named
/// for it. Skipped unless `SB_TOUR` is set (`TEST_RUNNER_SB_TOUR=1` from xcodebuild).
final class AppTourUITests: XCTestCase {
    private var app: XCUIApplication!
    private var part = "a"
    private var n = 0
    private let env = ProcessInfo.processInfo.environment

    override func setUp() {
        continueAfterFailure = true
    }

    private func shot(_ name: String) {
        n += 1
        usleep(700_000)
        let a = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        a.name = String(format: "%@%02d-%@", part, n, name)
        a.lifetime = .keepAlways
        add(a)
    }

    private func any(_ id: String) -> XCUIElement { app.descendants(matching: .any)[id] }

    @discardableResult
    private func tap(_ e: XCUIElement, _ what: String, timeout: TimeInterval = 12) -> Bool {
        guard e.waitForExistence(timeout: timeout) else {
            XCTFail("no \(what)")
            return false
        }
        e.tap()
        return true
    }

    private func back() {
        let b = app.buttons["back"].firstMatch
        if b.waitForExistence(timeout: 5) { b.tap() }
        usleep(500_000)
    }

    private func home() {
        for _ in 0..<5 where !app.buttons["micButton"].exists { back() }
    }

    private func start(_ letter: String, voice: String? = nil) throws {
        guard env["SB_TOUR"] != nil else { throw XCTSkip("set TEST_RUNNER_SB_TOUR=1") }
        part = letter
        app = XCUIApplication()
        if let voice { app.launchEnvironment["SB_FAKE_VOICE"] = voice }
        app.launch()
        XCTAssertTrue(app.buttons["micButton"].waitForExistence(timeout: 40), app.debugDescription)
        sleep(3)
    }

    /// Opens the space (`SB_TOUR_SPACE`, else the session's name) under Spaces, then the agent
    /// with this tab name.
    private func openAgent(session: String, agent: String) -> Bool {
        home()
        tap(app.buttons["Spaces"], "Spaces")
        let space = env["SB_TOUR_SPACE"] ?? session
        let row = app.buttons["space-\(space)"]
        for _ in 0..<4 where !row.isHittable { app.swipeUp() }
        guard tap(row, "space \(space)") else { return false }
        let label = app.staticTexts[agent].firstMatch
        for _ in 0..<5 where !(label.exists && label.isHittable) { app.swipeUp() }
        return tap(label, "agent \(agent)")
    }

    /// Home, settings, starting and sending, browsing, a session and an agent's pages.
    func testTourScreens() throws {
        try start("a", voice: "what is everyone working on right now")
        shot("home")
        app.swipeUp()
        shot("home-lists")
        app.swipeDown()

        if tap(app.buttons["devicesButton"], "settings") {
            sleep(2)
            shot("settings")
            app.swipeUp()
            shot("settings-more")
            app.swipeUp()
            shot("settings-end")
            tap(app.buttons["Done"], "Done")
        }

        if tap(app.buttons["startButton"], "start") {
            _ = app.buttons["submitStart"].waitForExistence(timeout: 10)
            sleep(1)
            shot("new-session")
            tap(app.buttons["Cancel"].firstMatch, "Cancel")
        }

        if tap(app.buttons["typeButton"], "type") {
            let field = any("composeField")
            if field.waitForExistence(timeout: 10) {
                shot("compose-empty")
                field.typeText("what is everyone working on right now")
                _ = app.staticTexts["previewMessage"].waitForExistence(timeout: 20)
                sleep(1)
                shot("compose-preview")
            }
            tap(app.buttons["Cancel"].firstMatch, "Cancel")
        }

        if tap(app.buttons["micButton"], "mic") {
            if app.staticTexts["transcript"].waitForExistence(timeout: 8) {
                sleep(1)
                shot("voice-listening")
            }
            if app.staticTexts["previewMessage"].waitForExistence(timeout: 25) {
                shot("voice-review")
            }
            tap(app.buttons["Cancel"].firstMatch, "Cancel")
        }

        if tap(app.buttons["Spaces"], "Spaces") {
            sleep(1)
            shot("all-sessions")
            app.swipeUp()
            shot("all-sessions-ended")
            app.swipeDown()
        }
        if tap(app.buttons["machinesLink"], "Machines") {
            sleep(2)
            shot("machines")
            let machine = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'machine-'")).firstMatch
            if machine.waitForExistence(timeout: 5) {
                machine.tap()
                sleep(1)
                shot("machine-sessions")
                back()
            }
            back()
        }
        if tap(app.buttons["Projects"], "Projects") {
            sleep(2)
            shot("projects")
            tap(app.buttons["Now"], "Now")
        }

        // A space, then one agent.
        home()
        let session = env["SB_TOUR_SESSION"] ?? "hc"
        tap(app.buttons["Spaces"], "Spaces")
        let row = app.buttons["space-\(env["SB_TOUR_SPACE"] ?? session)"]
        for _ in 0..<4 where !row.isHittable { app.swipeUp() }
        if tap(row, "space") {
            sleep(1)
            shot("session-spaces")
            app.swipeUp()
            shot("session-spaces-more")
            app.swipeDown()
        }
        if let agent = env["SB_TOUR_AGENT"], openAgent(session: session, agent: agent) {
            _ = any("latest").waitForExistence(timeout: 20)
            sleep(2)
            shot("agent-page")
            app.swipeUp()
            shot("agent-page-more")
            app.swipeDown()
            if tap(app.buttons["openConversation"], "conversation", timeout: 5) {
                _ = any("conversation").waitForExistence(timeout: 10)
                sleep(1)
                shot("conversation")
                tap(app.buttons["Summary"], "Summary")
            }
            if tap(app.buttons["openScreen"], "screen", timeout: 5) {
                _ = any("terminal").waitForExistence(timeout: 10)
                sleep(2)
                shot("terminal")
            }
            if tap(app.buttons["messageBar"], "message bar", timeout: 5) {
                let field = any("composeField")
                if field.waitForExistence(timeout: 8) {
                    shot("agent-compose")
                    field.typeText("/")
                    sleep(3)
                    shot("agent-skills")
                }
                tap(app.buttons["Cancel"].firstMatch, "Cancel")
            }
        }
        home()
    }

    /// A group of questions: tabs, a pick, Next, Back, and the step that would ask for Face ID.
    /// `SB_TOUR_PICKS` is the option to pick on each question, in order, separated by `|`.
    func testTourQuestionGroup() throws {
        try start(env["SB_TOUR_PART"] ?? "b")
        shot("home-agent-waiting")
        guard let agent = env["SB_TOUR_AGENT"], let picks = env["SB_TOUR_PICKS"]?.components(separatedBy: "|"),
              openAgent(session: env["SB_TOUR_SESSION"] ?? "hc", agent: agent) else { return }
        tap(app.buttons["answerDialog"], "answer", timeout: 30)
        guard any("dialog-card").waitForExistence(timeout: 30) else { return XCTFail("no dialog") }
        sleep(1)
        shot("question-first")
        let confirm = app.buttons["dialog-confirm"]
        for (i, pick) in picks.enumerated() {
            guard tap(app.buttons["option-\(pick)"], "option \(pick)", timeout: 20) else { return }
            shot("question-\(i + 1)-picked")
            if i == picks.count - 1 { break }
            confirm.tap()
            // The next question comes back with the answer.
            _ = app.buttons["option-\(picks[i + 1])"].waitForExistence(timeout: 20)
            sleep(1)
            shot("question-\(i + 2)")
            if i == 0, app.buttons["dialog-back"].waitForExistence(timeout: 5) {
                app.buttons["dialog-back"].tap()
                _ = app.buttons["option-\(pick)"].waitForExistence(timeout: 20)
                sleep(1)
                shot("question-back-to-1")
                tap(app.buttons["option-\(pick)"], "option \(pick) again")
                confirm.tap()
                _ = app.buttons["option-\(picks[1])"].waitForExistence(timeout: 20)
            }
        }
        // Claude ends with a review: Submit answers, and a Cancel that takes two taps.
        if env["SB_TOUR_REVIEW"] != nil {
            confirm.tap()
            let cancel = app.buttons["dialog-cancel"]
            guard cancel.waitForExistence(timeout: 20) else { return XCTFail("no review") }
            sleep(1)
            shot("review-submit")
            cancel.tap()
            shot("review-cancel-armed")
            cancel.tap()
            _ = any("dialog-card").waitForNonExistence(timeout: 20)
            sleep(1)
            shot("review-declined")
        }
        home()
    }

    /// A permission dialog: its options, the deny picked, and what shows once it is answered.
    func testTourPermission() throws {
        try start("d")
        guard let agent = env["SB_TOUR_AGENT"], openAgent(session: env["SB_TOUR_SESSION"] ?? "hc", agent: agent) else { return }
        tap(app.buttons["answerDialog"], "answer", timeout: 30)
        guard any("dialog-card").waitForExistence(timeout: 30) else { return XCTFail("no dialog") }
        sleep(1)
        shot("permission")
        let deny = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'option-No'")).firstMatch
        guard tap(deny, "deny option") else { return }
        shot("permission-deny-picked")
        app.buttons["dialog-confirm"].tap()
        _ = deny.waitForNonExistence(timeout: 20)
        sleep(1)
        shot("permission-answered")
        home()

        // An ended session Switchboard started, and its details.
        if let ended = env["SB_TOUR_ENDED"] {
            if tap(app.buttons["Spaces"], "Spaces") {
                let row = app.buttons["space-\(ended)"]
                for _ in 0..<6 where !(row.exists && row.isHittable) { app.swipeUp() }
                if tap(row, "ended session") {
                    sleep(1)
                    shot("ended-session")
                    let task = app.buttons.matching(NSPredicate(format: "label CONTAINS 'Ended'")).firstMatch
                    if task.waitForExistence(timeout: 5) {
                        task.tap()
                        sleep(2)
                        shot("session-detail")
                        app.swipeUp()
                        shot("session-detail-more")
                    }
                }
            }
            home()
        }
    }

    /// The first screen of a phone that isn't paired. Simulator only: it resets the app.
    func testTourPairing() throws {
        guard env["SB_TOUR_PAIRING"] != nil else { throw XCTSkip("set TEST_RUNNER_SB_TOUR_PAIRING=1 on a simulator") }
        part = "p"
        app = XCUIApplication()
        app.launchEnvironment["SB_RESET"] = "1"
        app.launch()
        XCTAssertTrue(app.staticTexts["pairTitle"].waitForExistence(timeout: 20))
        shot("pairing")
    }
}
