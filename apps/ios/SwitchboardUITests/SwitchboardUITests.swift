import XCTest

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
            let output = app.staticTexts["output"]
            XCTAssertTrue(output.waitForExistence(timeout: 20))
            let deny = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH 'deny-'")).firstMatch
            XCTAssertTrue(deny.waitForExistence(timeout: 20), "the runner asks to run the tests")
            XCTAssertTrue(app.descendants(matching: .any)["crates/core/src/retry.rs"].waitForExistence(timeout: 10), "changed files")
            snapshot(app, "session")
            deny.tap()
            XCTAssertTrue(deny.waitForNonExistence(timeout: 20), "the denied approval is gone")
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
