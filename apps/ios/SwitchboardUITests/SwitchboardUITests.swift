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
        app.launch()
        allowNotifications(app)

        let name = app.textFields["deviceName"]
        XCTAssertTrue(name.waitForExistence(timeout: 10))
        app.buttons["pairButton"].tap()
        // The script answers yes at once, so the fingerprint screen may only flash by.
        XCTAssertTrue(app.navigationBars["Switchboard"].waitForExistence(timeout: 60), app.debugDescription)
        allowNotifications(app)

        if let project = env["SB_EXPECT_PROJECT"] {
            app.buttons["startButton"].tap()
            XCTAssertTrue(app.navigationBars["New session"].waitForExistence(timeout: 10))
            XCTAssertTrue(app.staticTexts[project].waitForExistence(timeout: 10) || app.buttons.containing(NSPredicate(format: "label CONTAINS %@", project)).firstMatch.exists)
            app.buttons["Cancel"].tap()
        }

        app.buttons["devicesButton"].tap()
        XCTAssertTrue(app.staticTexts["this phone"].waitForExistence(timeout: 10))
        app.buttons["Done"].tap()
        XCTAssertTrue(app.navigationBars["Switchboard"].waitForExistence(timeout: 5))

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

    /// Accepts the notification permission alert if it shows.
    private func allowNotifications(_ app: XCUIApplication) {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let allow = springboard.buttons["Allow"]
        if allow.waitForExistence(timeout: 2) { allow.tap() }
    }
}
