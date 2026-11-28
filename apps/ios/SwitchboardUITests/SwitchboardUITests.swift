import XCTest

/// Home is up when its mic is on screen.
private func atHome(_ app: XCUIApplication, timeout: TimeInterval) -> Bool {
    app.buttons["micButton"].waitForExistence(timeout: timeout)
}

/// Back out of every pushed page.
private func goHome(_ app: XCUIApplication) {
    for _ in 0..<6 {
        let back = app.buttons["back"].firstMatch
        guard back.exists, back.isHittable else { return }
        back.tap()
        usleep(400_000)
    }
}

private extension XCUIElement {
    func waitForSelected(timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if isSelected { return true }
            usleep(200_000)
        }
        return isSelected
    }

    func waitForEnabled(timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if isEnabled { return true }
            usleep(200_000)
        }
        return isEnabled
    }
}

/// Runs against a real hub. `scripts/e2e.sh` starts one, runs `pair-device`, and passes the link
/// as `TEST_RUNNER_SB_PAIR_LINK`; without it the pairing test is skipped.
final class SwitchboardUITests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    func testPairingScreenShowsFirst() {
        let app = XCUIApplication()
        app.launchEnvironment["SB_RESET"] = "1"
        app.launch()
        XCTAssertTrue(app.staticTexts["pairTitle"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.buttons["pairButton"].isEnabled, "no link yet")
    }

    func testPairsWithTheHubThenShowsSessionsAndDevices() throws {
        let env = ProcessInfo.processInfo.environment
        guard let link = env["SB_PAIR_LINK"], !link.isEmpty else {
            throw XCTSkip("no SB_PAIR_LINK; run scripts/e2e.sh")
        }
        let app = XCUIApplication()
        app.launchEnvironment["SB_RESET"] = "1"
        app.launchEnvironment["SB_PAIR_LINK"] = link
        app.launchEnvironment["SB_FAKE_VOICE"] = "In demo run the benchmarks and compare them with main"
        app.launch()
        allowNotifications(app)

        let name = app.textFields["deviceName"]
        XCTAssertTrue(name.waitForExistence(timeout: 10))
        app.buttons["pairButton"].tap()
        // The script answers yes at once, so the fingerprint screen may only flash by.
        XCTAssertTrue(atHome(app, timeout: 60), app.debugDescription)
        allowNotifications(app)
        snapshot(app, "home")

        // Home's tabs follow a swipe across the list.
        app.swipeLeft()
        XCTAssertTrue(app.buttons["Spaces"].waitForSelected(timeout: 5), "a left swipe opens Spaces")
        app.swipeRight()
        XCTAssertTrue(app.buttons["Now"].waitForSelected(timeout: 5), "a right swipe goes back to Now")

        if let project = env["SB_EXPECT_PROJECT"] {
            app.buttons["startButton"].tap()
            XCTAssertTrue(app.buttons["submitStart"].waitForExistence(timeout: 10))
            XCTAssertTrue(app.buttons["backend-hc"].waitForExistence(timeout: 10), "where it opens")
            snapshot(app, "new-session")
            XCTAssertTrue(app.staticTexts[project].waitForExistence(timeout: 10) || app.buttons.containing(NSPredicate(format: "label CONTAINS %@", project)).firstMatch.exists)
            app.buttons["Cancel"].tap()

            // Typed text that names no project, with nothing current: the hub would ask, so the
            // sheet lists the projects and nothing is picked until the user picks one.
            app.buttons["typeButton"].tap()
            let field = app.descendants(matching: .any)["composeField"]
            XCTAssertTrue(field.waitForExistence(timeout: 10))
            field.typeText("fix the flaky test")
            let route = app.buttons["route-start:\(project)"]
            XCTAssertTrue(route.waitForExistence(timeout: 20), app.debugDescription)
            XCTAssertFalse(app.buttons["composeSend"].isEnabled, "nothing picked yet")
            route.tap()
            snapshot(app, "review-typed")
            app.buttons["composeSend"].tap()

            // The scripted runner starts it; the home screen says so and opens it.
            let sent = app.buttons["sentCard"]
            XCTAssertTrue(sent.waitForExistence(timeout: 30), app.debugDescription)
            sent.tap()
            // The latest turns come from the agent's conversation, not its terminal.
            let latest = app.descendants(matching: .any)["latest"]
            XCTAssertTrue(latest.waitForExistence(timeout: 20), app.debugDescription)
            XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'races the scheduler'")).firstMatch
                .waitForExistence(timeout: 20), "the latest reply")
            let deny = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'deny-'")).firstMatch
            XCTAssertTrue(deny.waitForExistence(timeout: 20), "the runner asks to run the tests")
            XCTAssertTrue(app.descendants(matching: .any)["crates/core/src/retry.rs"].waitForExistence(timeout: 10), "changed files")
            snapshot(app, "session")
            deny.tap()
            XCTAssertTrue(deny.waitForNonExistence(timeout: 20), "the denied approval is gone")

            // The whole conversation: its steps fold into one line that opens to them.
            app.buttons["openConversation"].tap()
            XCTAssertTrue(app.descendants(matching: .any)["conversation"].waitForExistence(timeout: 10))
            snapshot(app, "conversation")
            app.buttons.containing(NSPredicate(format: "label CONTAINS 'Working'")).firstMatch.tap()
            app.staticTexts["Edited crates/core/src/retry.rs"].tap()
            XCTAssertTrue(app.staticTexts["+6 −2"].waitForExistence(timeout: 5), "a tool opens to its result")

            // Typing / lists the agent's skills; picking one puts it in the message.
            app.buttons["messageBar"].tap()
            XCTAssertTrue(field.waitForExistence(timeout: 5))
            field.typeText("/co")
            let commit = app.buttons["skill-/commit"]
            XCTAssertTrue(commit.waitForExistence(timeout: 10), app.debugDescription)
            snapshot(app, "skills")
            commit.tap()
            XCTAssertTrue(app.buttons["skill-/commit"].waitForNonExistence(timeout: 5), "picked, the list goes")
            app.buttons["Cancel"].firstMatch.tap()

            app.buttons["Screen"].tap()
            XCTAssertTrue(app.descendants(matching: .any)["terminal"].waitForExistence(timeout: 10))
            snapshot(app, "terminal")
            // The system's edge swipe goes back, even from the last of the page's tabs.
            app.coordinate(withNormalizedOffset: CGVector(dx: 0, dy: 0.5))
                .press(forDuration: 0.05, thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)))
            XCTAssertTrue(atHome(app, timeout: 10), "an edge swipe goes back")
            snapshot(app, "home-running")

            // Voice: the scripted words name the project, so the hub's preview picks a start there.
            app.buttons["micButton"].tap()
            let transcript = app.staticTexts["transcript"]
            XCTAssertTrue(transcript.waitForExistence(timeout: 5))
            sleep(1)
            snapshot(app, "listening")
            let preview = app.staticTexts["previewMessage"]
            XCTAssertTrue(preview.waitForExistence(timeout: 20), app.debugDescription)
            XCTAssertTrue(preview.label.contains("Starts a new session in \(project)"), preview.label)
            XCTAssertTrue(app.buttons["composeSend"].isEnabled)
            snapshot(app, "review-voice")
            app.buttons["Cancel"].firstMatch.tap()
        }

        app.buttons["devicesButton"].tap()
        XCTAssertTrue(app.staticTexts["this phone"].waitForExistence(timeout: 10))
        app.buttons["Done"].tap()
        XCTAssertTrue(atHome(app, timeout: 5))

        // Spaces lists the Herdr app's spaces and the Herdr terminal's workspaces alike.
        app.buttons["Spaces"].tap()
        let imports = app.buttons["space-crm › Imports"]
        XCTAssertTrue(imports.waitForExistence(timeout: 15), app.debugDescription)
        XCTAssertTrue(app.buttons["space-atlas-web"].exists, "a Herdr app space")
        snapshot(app, "spaces")
        imports.tap()
        XCTAssertTrue(app.staticTexts["orchestrator"].waitForExistence(timeout: 10), app.debugDescription)
        snapshot(app, "space")
        app.staticTexts["orchestrator"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["latest"].waitForExistence(timeout: 10), "tapping an agent opens its page")
        // Its question opens as a page; the free-text row is shown but can't be picked.
        let answer = app.buttons["answerDialog"]
        XCTAssertTrue(answer.waitForExistence(timeout: 15), app.debugDescription)
        snapshot(app, "agent-waiting")
        answer.tap()
        XCTAssertTrue(app.buttons["option-Blue"].waitForExistence(timeout: 15), app.debugDescription)
        XCTAssertTrue(app.buttons["option-Blue"].isEnabled)
        XCTAssertFalse(app.buttons["option-Type something."].isEnabled)
        snapshot(app, "agent-question")
        goHome(app)

        // The Codex agent, in the numbered workspace, waits at a permission dialog; its deny needs no Face ID.
        app.buttons["Spaces"].tap()
        app.buttons["space-crm"].tap()
        XCTAssertTrue(app.staticTexts["Review the import"].waitForExistence(timeout: 10), "an unnamed agent goes by its title")
        app.staticTexts["Review the import"].tap()
        let answerCodex = app.buttons["answerDialog"]
        XCTAssertTrue(answerCodex.waitForExistence(timeout: 15), app.debugDescription)
        answerCodex.tap()
        let no = app.buttons["option-No, and tell Codex what to do differently (esc)"]
        XCTAssertTrue(no.waitForExistence(timeout: 15), app.debugDescription)
        XCTAssertTrue(app.buttons["option-Yes, and don't ask again (p)"].isEnabled, "a standing approval is offered")
        let confirm = app.buttons["dialog-confirm"]
        XCTAssertFalse(confirm.isEnabled, "nothing is sent before an option is picked")
        snapshot(app, "agent-permission")
        no.tap()
        XCTAssertTrue(no.exists, "picking an option sends nothing yet")
        confirm.tap()
        XCTAssertTrue(no.waitForNonExistence(timeout: 20), "the answered dialog is gone")
        snapshot(app, "agent-answered")
        goHome(app)

        // A Herdr app space takes a new session from its page, with the space given and not chosen.
        app.buttons["Spaces"].tap()
        let atlas = app.buttons["space-atlas-web"]
        XCTAssertTrue(atlas.waitForExistence(timeout: 15), app.debugDescription)
        atlas.tap()
        let here = app.buttons["newAgentHere"]
        XCTAssertTrue(here.waitForExistence(timeout: 10), app.debugDescription)
        here.tap()
        XCTAssertTrue(app.descendants(matching: .any)["startInto"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertFalse(app.buttons["backend-herdr"].exists, "no choice of where it opens")
        snapshot(app, "start-into-space")
        app.buttons["Cancel"].firstMatch.tap()

        // Renaming it: a name another space has is refused in words, and the page follows a good one.
        let rename = app.buttons["rename"]
        for _ in 0..<3 where !(rename.exists && rename.isHittable) { app.swipeUp() }
        XCTAssertTrue(rename.waitForExistence(timeout: 5), app.debugDescription)
        rename.tap()
        let nameField = app.textFields["renameField"]
        XCTAssertTrue(nameField.waitForExistence(timeout: 5), app.debugDescription)
        let erase = { (n: Int) in String(repeating: XCUIKeyboardKey.delete.rawValue, count: n) }
        let caretAtEnd = { nameField.coordinate(withNormalizedOffset: CGVector(dx: 0.95, dy: 0.5)).tap() }
        XCTAssertFalse(app.buttons["renameSave"].isEnabled, "the same name is nothing to save")
        caretAtEnd()
        nameField.typeText(erase("atlas-web".count) + "taken")
        app.buttons["renameSave"].tap()
        let refused = app.staticTexts["renameError"]
        XCTAssertTrue(refused.waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertEqual(refused.label, "That name is taken.")
        snapshot(app, "rename-taken")
        caretAtEnd()
        nameField.typeText(erase("taken".count) + "web")
        app.buttons["renameSave"].tap()
        XCTAssertTrue(nameField.waitForNonExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.staticTexts["search"].waitForExistence(timeout: 10), "the page is the same space, under its new name")
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label BEGINSWITH 'web.'")).firstMatch.waitForExistence(timeout: 5),
                      app.debugDescription)
        snapshot(app, "space-renamed")

        // One of its agents was named when it started: the phone says so and offers no rename.
        app.staticTexts["docs"].tap()
        let desk = app.buttons["deskToggle"]
        for _ in 0..<4 where !(desk.exists && desk.isHittable) { app.swipeUp() }
        desk.tap()
        let fixed = app.staticTexts["renameBlocked"]
        for _ in 0..<4 where !fixed.exists { app.swipeUp() }
        XCTAssertTrue(fixed.waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertEqual(fixed.label, "Rename it at the Mac.")
        XCTAssertFalse(app.buttons["rename"].exists)
        snapshot(app, "agent-name-fixed")
        goHome(app)
        app.buttons["Now"].tap()

        if env["SB_EXPECT_PROJECT"] != nil { try cloudVoice(app) }

        if env["SB_EXPECT_PUSH"] != nil {
            // The script pairs a second device; the hub pushes "New device paired" through simctl.
            // The banner may have come and gone, so look in Notification Center.
            let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
            let note = springboard.descendants(matching: .any)
                .matching(NSPredicate(format: "label CONTAINS[c] %@", "New device paired")).firstMatch
            let deadline = Date().addingTimeInterval(90)
            var found = false
            while !found && Date() < deadline {
                let top = springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.001))
                top.press(forDuration: 0.1, thenDragTo: springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.6)))
                found = note.waitForExistence(timeout: 5)
                // Later pushes stack on top of it, and only the newest is in the tree until a tap expands the stack.
                let stack = springboard.descendants(matching: .any).matching(NSPredicate(
                    format: "identifier == 'NotificationShortLookView' AND label CONTAINS 'Grouped'")).firstMatch
                if !found && stack.exists { stack.tap(); found = note.waitForExistence(timeout: 5) }
                if !found { XCUIDevice.shared.press(.home); app.activate() }
            }
            XCTAssertTrue(found, "the push reached the simulator")
        }
    }

    /// The app again, still paired, with canned transcriptions: a success, then a failure with
    /// Retry, with typing kept while the Retry runs. The recording itself is scripted too.
    private func cloudVoice(_ app: XCUIApplication) throws {
        app.terminate()
        app.launchEnvironment["SB_RESET"] = nil
        app.launchEnvironment["SB_PAIR_LINK"] = nil
        app.launchEnvironment["SB_FAKE_VOICE"] = nil
        app.launchEnvironment["SB_FAKE_TRANSCRIBE"] = "In demo run the benchmarks|error:server|and compare them with main"
        app.launchEnvironment["SB_FAKE_TRANSCRIBE_MS"] = "6000"
        app.launch()
        XCTAssertTrue(atHome(app, timeout: 30), app.debugDescription)

        // A saved key is never shown; removing it brings the field back.
        app.buttons["devicesButton"].tap()
        let keyField = app.secureTextFields["voiceKeyField"]
        for _ in 0..<4 where !keyField.exists { app.swipeUp() }
        XCTAssertTrue(keyField.waitForExistence(timeout: 5), app.debugDescription)
        keyField.tap()
        keyField.typeText("sk-fake-ui-test-key")
        app.buttons["voiceKeySave"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["voiceKeySaved"].waitForExistence(timeout: 5), app.debugDescription)
        XCTAssertFalse(app.staticTexts.containing(NSPredicate(format: "label CONTAINS 'sk-fake'")).firstMatch.exists)
        app.buttons["voiceKeyRemove"].tap()
        XCTAssertTrue(keyField.waitForExistence(timeout: 5), app.debugDescription)
        app.buttons["Done"].tap()

        app.buttons["micButton"].tap()
        let status = app.staticTexts["transcript"]
        XCTAssertTrue(status.waitForExistence(timeout: 5))
        XCTAssertTrue(status.label.contains("Recording for OpenAI"), status.label)
        let uploading = NSPredicate(format: "label CONTAINS 'Transcribing with OpenAI'")
        expectation(for: uploading, evaluatedWith: status)
        waitForExpectations(timeout: 15)
        let field = app.descendants(matching: .any)["composeField"]
        XCTAssertTrue(field.waitForExistence(timeout: 30), app.debugDescription)
        let send = app.buttons["composeSend"]
        XCTAssertEqual(field.value as? String, "In demo run the benchmarks")
        XCTAssertTrue(app.staticTexts["previewMessage"].waitForExistence(timeout: 20), app.debugDescription)
        snapshot(app, "review-cloud")

        // Keep talking: this upload fails, and the words so far stay.
        app.buttons["Keep talking"].tap()
        let retry = app.buttons["voiceRetry"]
        XCTAssertTrue(retry.waitForExistence(timeout: 30), app.debugDescription)
        XCTAssertEqual(field.value as? String, "In demo run the benchmarks")
        snapshot(app, "review-retry")

        // Retry: Send waits, and what's typed meanwhile stays, with the result after it.
        retry.tap()
        XCTAssertTrue(app.descendants(matching: .any)["voiceRetrying"].waitForExistence(timeout: 2), app.debugDescription)
        XCTAssertFalse(send.isEnabled, "Send waits for the Retry")
        field.tap()
        field.typeText(" please")
        XCTAssertTrue(retry.waitForNonExistence(timeout: 1))
        let done = NSPredicate(format: "value CONTAINS 'please' AND value CONTAINS 'and compare them with main' AND value CONTAINS 'run the benchmarks'")
        expectation(for: done, evaluatedWith: field)
        waitForExpectations(timeout: 20)
        XCTAssertTrue(send.waitForEnabled(timeout: 20), app.debugDescription)
        app.buttons["Cancel"].firstMatch.tap()
        XCTAssertTrue(atHome(app, timeout: 5))
    }

    /// A screenshot kept in the result bundle, for checking the design by eye.
    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    /// Accepts the notification permission alert if it shows.
    private func allowNotifications(_ app: XCUIApplication) {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let allow = springboard.buttons["Allow"]
        if allow.waitForExistence(timeout: 2) { allow.tap() }
    }
}
