import XCTest

private extension XCUIElement {
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
        XCTAssertTrue(app.navigationBars["Pair with your hub"].waitForExistence(timeout: 10))
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
        XCTAssertTrue(app.navigationBars["Agents"].waitForExistence(timeout: 60), app.debugDescription)
        allowNotifications(app)
        snapshot(app, "home")

        if let project = env["SB_EXPECT_PROJECT"] {
            app.buttons["startButton"].tap()
            XCTAssertTrue(app.navigationBars["New session"].waitForExistence(timeout: 10))
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
            app.navigationBars.buttons.element(boundBy: 0).tap()
            app.navigationBars.buttons.element(boundBy: 0).tap()
            XCTAssertTrue(app.navigationBars["Agents"].waitForExistence(timeout: 10))
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
        XCTAssertTrue(app.navigationBars["Agents"].waitForExistence(timeout: 5))

        // A Herdr session lists its agents, whoever started them.
        let crm = app.buttons["session-crm"]
        XCTAssertTrue(crm.waitForExistence(timeout: 15), app.debugDescription)
        snapshot(app, "home")
        crm.tap()
        XCTAssertTrue(app.staticTexts["Imports › orchestrator"].waitForExistence(timeout: 10), app.debugDescription)
        XCTAssertTrue(app.staticTexts["Review the import"].exists, "an unnamed agent goes by its title, and a numbered space is left out")
        snapshot(app, "session")
        app.staticTexts["Imports › orchestrator"].tap()
        XCTAssertTrue(app.descendants(matching: .any)["latest"].waitForExistence(timeout: 10), "tapping an agent opens its page")
        // Its question shows as options; the free-text row is shown but can't be picked.
        XCTAssertTrue(app.descendants(matching: .any)["dialog-card"].waitForExistence(timeout: 15), app.debugDescription)
        XCTAssertTrue(app.buttons["option-Blue"].isEnabled)
        XCTAssertFalse(app.buttons["option-Type something."].isEnabled)
        snapshot(app, "agent-question")
        app.navigationBars.buttons.firstMatch.tap()

        // The Codex agent waits at a permission dialog; its deny needs no Face ID.
        app.staticTexts["Review the import"].tap()
        let no = app.buttons["option-No, and tell Codex what to do differently (esc)"]
        XCTAssertTrue(no.waitForExistence(timeout: 15), app.debugDescription)
        XCTAssertFalse(app.buttons["option-Yes, and don't ask again (p)"].isEnabled, "a standing approval isn't offered")
        snapshot(app, "agent-permission")
        no.tap()
        XCTAssertTrue(no.waitForNonExistence(timeout: 20), "the answered dialog is gone")
        app.navigationBars.buttons.firstMatch.tap()
        app.navigationBars.buttons.firstMatch.tap()

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
        XCTAssertTrue(app.navigationBars["Agents"].waitForExistence(timeout: 30), app.debugDescription)

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
        XCTAssertTrue(app.navigationBars["Agents"].waitForExistence(timeout: 5))
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
