import XCTest

/// Pairs with a throwaway hub and its scripted runner, then keeps a screenshot of each main
/// screen in light and dark, for comparing with the design. Skipped unless `SB_SHOTS` is set with
/// `SB_PAIR_LINK`, as `scripts/shots.sh` sets them.
final class ScreenshotsUITests: XCTestCase {
    private var app: XCUIApplication!
    private var appearance = "light"

    override func setUp() {
        continueAfterFailure = true
    }

    func testScreens() throws {
        let env = ProcessInfo.processInfo.environment
        guard env["SB_SHOTS"] != nil, let link = env["SB_PAIR_LINK"], !link.isEmpty else {
            throw XCTSkip("run scripts/shots.sh")
        }
        app = XCUIApplication()
        app.launchEnvironment["SB_RESET"] = "1"
        app.launchEnvironment["SB_PAIR_LINK"] = link
        app.launchArguments = ["-appearance", "light"]
        app.launch()
        allowNotifications()
        XCTAssertTrue(app.buttons["pairButton"].waitForExistence(timeout: 30))
        shot("pair")
        app.buttons["pairButton"].tap()
        XCTAssertTrue(app.buttons["micButton"].waitForExistence(timeout: 60), app.debugDescription)
        allowNotifications()

        // A session in the Herdr app, so Now has an approval and Spaces a session of Switchboard's.
        app.buttons["startButton"].tap()
        let prompt = app.textFields["promptField"]
        XCTAssertTrue(prompt.waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["backend-hc"].waitForExistence(timeout: 10))
        prompt.tap()
        prompt.typeText("Fix the flaky retry test and tell me why it failed")
        app.buttons["submitStart"].tap()
        XCTAssertTrue(app.buttons["stopOrDelete"].waitForExistence(timeout: 30), app.debugDescription)
        _ = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'deny-'")).firstMatch.waitForExistence(timeout: 15)
        back()

        for mode in ["light", "dark"] {
            appearance = mode
            app.terminate()
            app.launchEnvironment["SB_RESET"] = nil
            app.launchEnvironment["SB_PAIR_LINK"] = nil
            app.launchEnvironment["SB_FAKE_VOICE"] = "In demo, fix the flaky retry test"
            app.launchArguments = ["-appearance", mode]
            app.launch()
            XCTAssertTrue(app.buttons["micButton"].waitForExistence(timeout: 30))
            sleep(4)
            tour()
        }
    }

    private func tour() {
        shot("home-now")
        tap(app.buttons["Spaces"])
        shot("home-spaces")
        if tap(app.buttons["space-atlas-web"]) {
            shot("space")
            if tap(app.staticTexts["search"]) {
                shot("agent-summary")
                tap(app.buttons["Conversation"])
                sleep(1)
                shot("agent-conversation")
                tap(app.buttons["Screen"])
                sleep(2)
                shot("agent-screen")
                tap(app.buttons["messageBar"])
                let field = app.descendants(matching: .any)["composeField"]
                if field.waitForExistence(timeout: 5) {
                    field.typeText("/co")
                    _ = app.buttons["skill-/commit"].waitForExistence(timeout: 10)
                    shot("skills")
                    app.buttons["Cancel"].firstMatch.tap()
                }
                back()
            }
            back()
        }
        if tap(app.buttons["machinesLink"]) {
            sleep(1)
            shot("machines")
            back()
        }
        tap(app.buttons["Projects"])
        shot("home-projects")
        tap(app.buttons["Now"])

        // The session Switchboard started: approval, changes, actions, then stop or delete.
        let managed = app.buttons.matching(NSPredicate(format: "label CONTAINS 'wants to run'")).firstMatch
        if managed.waitForExistence(timeout: 5) {
            managed.tap()
            if app.buttons["stopOrDelete"].waitForExistence(timeout: 10) {
                sleep(2)
                shot("managed")
                app.buttons["stopOrDelete"].tap()
                sleep(1)
                shot("lifecycle")
                app.buttons["Cancel"].firstMatch.tap()
                // A changed file's diff, selecting lines, and asking about them.
                let file = app.buttons["crates/core/src/retry.rs"]
                for _ in 0..<4 where !file.isHittable { app.swipeUp() }
                if tap(file) {
                    sleep(2)
                    shot("diff")
                    if tap(app.buttons["Line 42"]), tap(app.buttons["Line 43"]) {
                        shot("diff-selected")
                        if tap(app.buttons["askAgent"]) {
                            sleep(1)
                            app.textViews["askQuestion"].typeText("Cap the delay at 30 seconds.")
                            shot("ask")
                            app.buttons["Cancel"].firstMatch.tap()
                        }
                    }
                    back()
                }
                let binary = app.buttons["docs/rate-limit.png"]
                for _ in 0..<4 where !binary.isHittable { app.swipeUp() }
                if tap(binary) {
                    sleep(2)
                    shot("diff-binary")
                    back()
                }
                // Denied, the agent finishes; its ship buttons show, then the confirm sheet.
                app.swipeDown()
                if tap(app.buttons["Deny"].firstMatch) {
                    let ship = app.buttons["ship-/pr"]
                    if ship.waitForExistence(timeout: 15) {
                        for _ in 0..<4 where !ship.isHittable { app.swipeUp() }
                        shot("ship")
                        // A ship skill that ends without a pull request.
                        if tap(app.buttons["runAnotherSkill"]), tap(app.buttons["/ship"]), tap(app.buttons["shipRun"]) {
                            if app.staticTexts["No pull request was opened. See the agent's reply."].waitForExistence(timeout: 20) {
                                sleep(1)
                                shot("ship-no-pr")
                            }
                        }
                        // Then /pr, whose reply names one.
                        let pr = app.buttons["ship-/pr"]
                        for _ in 0..<4 where pr.exists && !pr.isHittable { app.swipeUp() }
                        if tap(pr) {
                            app.textFields["shipInstructions"].tap()
                            app.textFields["shipInstructions"].typeText("Draft, base main")
                            shot("ship-confirm")
                            if tap(app.buttons["shipRun"]), app.buttons["openPullRequest"].waitForExistence(timeout: 20) {
                                sleep(1)
                                for _ in 0..<3 where !app.buttons["openPullRequest"].isHittable { app.swipeUp() }
                                shot("ship-pr")
                            }
                        }
                    }
                }
                back()
            }
        }

        // Decisions: the question group and the permission.
        let question = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'needs-crm › Imports'")).firstMatch
        if tap(question) {
            _ = app.buttons["option-Blue"].waitForExistence(timeout: 10)
            app.buttons["option-Blue"].tap()
            shot("question")
            back()
        }
        let permission = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'needs-crm › Review'")).firstMatch
        if tap(permission) {
            let no = app.buttons["option-No, and tell Codex what to do differently (esc)"]
            if no.waitForExistence(timeout: 10) { no.tap() }
            shot("permission")
            back()
        }

        // New session, then the composer, listening and review.
        tap(app.buttons["startButton"])
        if app.buttons["submitStart"].waitForExistence(timeout: 10) {
            sleep(2)
            shot("new-session")
            app.buttons["Cancel"].firstMatch.tap()
        }
        tap(app.buttons["typeButton"])
        let field = app.descendants(matching: .any)["composeField"]
        if field.waitForExistence(timeout: 5) {
            field.typeText("fix the flaky test")
            _ = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'route-'")).firstMatch.waitForExistence(timeout: 15)
            shot("compose")
            app.buttons["Cancel"].firstMatch.tap()
        }
        tap(app.buttons["micButton"])
        if app.staticTexts["transcript"].waitForExistence(timeout: 5) {
            usleep(800_000)
            shot("listening")
            if app.staticTexts["previewMessage"].waitForExistence(timeout: 20) { shot("voice-review") }
            app.buttons["Cancel"].firstMatch.tap()
        }

        tap(app.buttons["devicesButton"])
        if app.staticTexts["this phone"].waitForExistence(timeout: 10) {
            shot("settings")
            app.swipeUp()
            shot("settings-more")
            // Settings › Lock screen: when the card shows, both choices.
            let working = app.buttons["liveMode-working"]
            for _ in 0..<4 where !working.isHittable { app.swipeUp() }
            if working.exists {
                shot("settings-lock")
                app.buttons["liveMode-always"].tap()
                sleep(1)
                shot("settings-lock-always")
                working.tap()
                app.swipeDown(); app.swipeDown()
            }
            // Settings › Ship actions, with two buttons added from the agent's skills.
            let ship = app.buttons["shipSettings"]
            for _ in 0..<3 where !ship.isHittable { app.swipeUp() }
            if tap(ship) {
                sleep(3)
                for name in ["/pr", "/commit", "/code-review", "/triage-reviews"] {
                    let add = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", name + ",")).firstMatch
                    if add.waitForExistence(timeout: 5) { add.tap(); usleep(800_000) }
                }
                shot("ship-settings")
                app.buttons["Settings"].firstMatch.tap()
                sleep(1)
            }
            app.buttons["Done"].tap()
        }
        // Coming back after two hours: the Inbox card, then one agent's "Since you looked".
        app.terminate()
        app.launchEnvironment["SB_AWAY_SINCE_MIN"] = "120"
        app.launch()
        if app.otherElements["awayCard"].waitForExistence(timeout: 20) || app.buttons["markAwaySeen"].waitForExistence(timeout: 5) {
            sleep(2)
            shot("away-inbox")
            let row = app.buttons.matching(NSPredicate(format: "label CONTAINS 'demo'")).firstMatch
            if tap(row), app.otherElements["sinceYouLooked"].waitForExistence(timeout: 10) || app.staticTexts["Facts the hub recorded, plus the agent's own last reply. No summarising model."].waitForExistence(timeout: 5) {
                sleep(1)
                shot("away-agent")
            }
        }
    }

    @discardableResult
    private func tap(_ e: XCUIElement) -> Bool {
        guard e.waitForExistence(timeout: 10) else { return false }
        e.tap()
        usleep(600_000)
        return true
    }

    private func back() {
        let b = app.buttons["back"].firstMatch
        if b.waitForExistence(timeout: 3), b.isHittable { b.tap() }
        usleep(500_000)
    }

    private func shot(_ name: String) {
        usleep(600_000)
        let a = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        a.name = "\(appearance)-\(name)"
        a.lifetime = .keepAlways
        add(a)
    }

    private func allowNotifications() {
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Allow"]
        if allow.waitForExistence(timeout: 2) { allow.tap() }
    }

    /// A read-only pass over a phone already paired to a real hub: it never resets or pairs, and
    /// only navigates. Typed and scripted-voice text is cancelled; nothing is sent, started,
    /// answered or changed. Skipped unless `SB_OWNER_TOUR` is set; `SB_OWNER_APPEARANCE` and
    /// `SB_OWNER_TEXT` (a content size category) apply to this launch only.
    func testOwnersHub() throws {
        let env = ProcessInfo.processInfo.environment
        guard env["SB_OWNER_TOUR"] != nil else { throw XCTSkip("set TEST_RUNNER_SB_OWNER_TOUR=1") }
        appearance = [env["SB_OWNER_APPEARANCE"] ?? "light", env["SB_OWNER_TEXT"].map { _ in "large" }].compactMap { $0 }.joined(separator: "-")
        app = XCUIApplication()
        app.launchEnvironment["SB_FAKE_VOICE"] = "what is everyone working on right now"
        app.launchArguments = ["-appearance", env["SB_OWNER_APPEARANCE"] ?? "light"]
        if let size = env["SB_OWNER_TEXT"] { app.launchArguments += ["-UIPreferredContentSizeCategoryName", size] }
        app.launch()
        XCTAssertTrue(app.buttons["micButton"].waitForExistence(timeout: 60), app.debugDescription)
        sleep(6)
        shot("home-now")
        app.swipeUp(); shot("home-now-more"); app.swipeDown()

        tap(app.buttons["Spaces"])
        sleep(2)
        shot("home-spaces")
        app.swipeUp(); shot("home-spaces-more"); app.swipeDown()
        let spaces = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'space-'"))
        let inApp = spaces.matching(NSPredicate(format: "label CONTAINS 'Herdr app'")).firstMatch
        if tap(inApp) {
            sleep(2)
            shot("space-app")
            app.swipeUp(); shot("space-app-desk"); app.swipeDown()
            if tap(app.scrollViews.firstMatch.buttons.firstMatch) {
                sleep(4)
                shot("agent-summary")
                app.swipeUp(); shot("agent-summary-desk"); app.swipeDown()
                tap(app.buttons["Conversation"]); sleep(4); shot("agent-conversation")
                tap(app.buttons["Screen"]); sleep(4); shot("agent-screen")
                tap(app.buttons["Summary"])
                back()
            }
            back()
        }
        let workspace = spaces.matching(NSPredicate(format: "identifier CONTAINS '›' AND label CONTAINS 'Herdr terminal'")).firstMatch
        if tap(workspace) {
            sleep(2)
            shot("space-terminal-workspace")
            app.swipeUp(); shot("space-terminal-workspace-desk"); app.swipeDown()
            back()
        }
        let terminal = spaces.matching(NSPredicate(format: "NOT (identifier CONTAINS '›') AND label CONTAINS 'Herdr terminal'")).firstMatch
        if tap(terminal) {
            sleep(2)
            shot("space-terminal")
            if tap(app.scrollViews.firstMatch.buttons.firstMatch) {
                sleep(4)
                shot("terminal-agent-summary")
                app.swipeUp(); shot("terminal-agent-desk"); app.swipeDown()
                back()
            }
            back()
        }
        if tap(app.buttons["machinesLink"]) {
            sleep(2)
            shot("machines")
            if tap(app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'machine-'")).firstMatch) {
                sleep(1)
                shot("machine-spaces")
                back()
            }
            back()
        }
        tap(app.buttons["Projects"])
        sleep(1)
        shot("home-projects")
        tap(app.buttons["Now"])

        // New session against this hub: look, then Cancel. Start is never pressed.
        if tap(app.buttons["startButton"]), app.buttons["submitStart"].waitForExistence(timeout: 10) {
            sleep(4)
            shot("new-session")
            app.swipeUp(); shot("new-session-more")
            app.buttons["Cancel"].firstMatch.tap()
        }
        // A typed preview and a scripted listen, both cancelled.
        if tap(app.buttons["typeButton"]) {
            let field = app.descendants(matching: .any)["composeField"]
            if field.waitForExistence(timeout: 10) {
                field.typeText("what is everyone working on right now")
                _ = app.staticTexts["previewMessage"].waitForExistence(timeout: 20)
                sleep(1)
                shot("compose")
            }
            app.buttons["Cancel"].firstMatch.tap()
        }
        if tap(app.buttons["micButton"]) {
            if app.staticTexts["transcript"].waitForExistence(timeout: 8) { usleep(700_000); shot("listening") }
            if app.staticTexts["previewMessage"].waitForExistence(timeout: 25) { shot("voice-review") }
            app.buttons["Cancel"].firstMatch.tap()
        }
        XCTAssertTrue(app.buttons["micButton"].waitForExistence(timeout: 10), "back at home")

        // Settings: look only.
        if tap(app.buttons["devicesButton"]) {
            sleep(2)
            shot("settings")
            app.swipeUp(); shot("settings-more")
            app.swipeUp(); shot("settings-end")
            app.buttons["Done"].tap()
        }
    }
}
