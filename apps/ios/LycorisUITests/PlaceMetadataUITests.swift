import XCTest

/// A real failure (not an environment skip) when an expected fixture write never
/// arrives, so a broken serialization path cannot be silently skipped.
private enum WriteWaitError: Error { case missingWrite }

/// UI coverage for the venue tag and opening status. Fixture-backed cases run
/// only against the identified loopback `lycoris-place-metadata` fixture in the
/// Test configuration; no real account or production write is used.
@MainActor final class PlaceMetadataUITests: LocalBackendTestCase {
  private let base = URL(string: "http://127.0.0.1:8080")!
  private let fixtureAccount = (username: "ios_metadata_fixture", password: "Metadata-Fixture-1")
  override func setUp() { continueAfterFailure = false }

  // MARK: - Real list -> detail venue tag and closing-soon reminder

  func testListAndDetailShowVenueTagAndClosingSoon() async throws {
    try await resetFixture()
    let app = launch(now: "2026-09-20T13:45:00Z")
    let search = app.textFields["map.search"]
    XCTAssertTrue(search.waitForExistence(timeout: 10))
    search.tap()
    search.typeText("Metro Accessible Toilet")
    let row = app.buttons["place.row.21"]
    XCTAssertTrue(row.waitForExistence(timeout: 8))
    attach(app, "metadata-list")

    // The real, unique row button carries the complete label: category, name,
    // venue, hours and the real closing-soon status. The decorative tag is
    // reviewed from the screenshot, not required as a separate AX element.
    XCTAssertTrue(row.isHittable)
    XCTAssertEqual(app.buttons.matching(identifier: "place.row.21").count, 1)
    XCTAssertTrue(row.label.contains("Accessible Toilets"), row.label)
    XCTAssertTrue(row.label.contains("Metro Accessible Toilet"), row.label)
    XCTAssertTrue(row.label.contains("09:00–22:00"), row.label)
    XCTAssertTrue(row.label.contains("Closing soon"), row.label)

    row.tap()
    XCTAssertTrue(app.staticTexts["place.title"].waitForExistence(timeout: 8))
    attach(app, "metadata-detail")
    // The detail shows the localized venue tag and the short reminder.
    let venue = app.descendants(matching: .any)["place.venue"]
    XCTAssertTrue(venue.waitForExistence(timeout: 5))
    XCTAssertEqual(venue.label, "Metro")
    let closing = app.descendants(matching: .any)["place.closing-soon"]
    XCTAssertTrue(closing.waitForExistence(timeout: 5))
    XCTAssertEqual(closing.label, "Closing soon")
  }

  func testAirportAndPublicToiletTagsInListAndDetail() async throws {
    try await resetFixture()
    for (id, title, language, label) in [
      (24, "Airport Accessible Toilet", "en", "Airport"),
      (25, "Public Accessible Toilet", "zh", "公共卫生间"),
    ] {
      let app = launch(language: language, now: "2026-09-20T03:00:00Z")
      let search = app.textFields["map.search"]
      XCTAssertTrue(search.waitForExistence(timeout: 10))
      search.tap()
      search.typeText(title)
      let row = app.buttons["place.row.\(id)"]
      XCTAssertTrue(row.waitForExistence(timeout: 8))
      XCTAssertTrue(row.label.contains(label), row.label)
      attach(app, "metadata-new-venue-\(id)-list")
      row.tap()
      XCTAssertTrue(app.staticTexts["place.title"].waitForExistence(timeout: 8))
      let venue = app.descendants(matching: .any)["place.venue"]
      XCTAssertTrue(venue.waitForExistence(timeout: 5))
      XCTAssertEqual(venue.label, label)
      attach(app, "metadata-new-venue-\(id)-detail")
      app.terminate()
    }
  }

  func testNonToiletRowHasNoInventedVenueTag() async throws {
    try await resetFixture()
    let app = launch(now: "2026-09-20T03:00:00Z")
    let search = app.textFields["map.search"]
    XCTAssertTrue(search.waitForExistence(timeout: 10))
    search.tap()
    search.typeText("Nursing Room No Tag")
    let nursing = app.buttons["place.row.23"]
    XCTAssertTrue(nursing.waitForExistence(timeout: 8))
    XCTAssertFalse(nursing.label.contains("Other"))
    XCTAssertEqual(app.buttons.matching(identifier: "place.row.21").count, 0)
  }

  // MARK: - Accessibility sizes

  func testEnglishStandardAndChineseAccessibilityText() async throws {
    try await resetFixture()
    for (name, language, large, locale) in [
      ("en", "en", false, false), ("zh-AXXXL", "zh", true, true),
    ] {
      let app = launch(language: language, large: large, locale: locale, now: "2026-09-20T13:45:00Z")
      // This case verifies detail typography. Expand first so the search field
      // is stationary before typing; collapsed-row gestures have their own suite.
      let handle = app.buttons["map.panel.handle"]
      let expandedValue = language == "zh" ? "已展开" : "Expanded"
      XCTAssertTrue(handle.waitForExistence(timeout: 10), name)
      for _ in 0..<2 {
        if handle.value as? String == expandedValue { break }
        handle.tap()
      }
      let expanded = XCTNSPredicateExpectation(
        predicate: NSPredicate(format: "value == %@", expandedValue), object: handle)
      guard await XCTWaiter.fulfillment(of: [expanded], timeout: 5) == .completed else {
        XCTFail("Search panel did not expand: \(name)")
        return
      }
      let search = app.textFields["map.search"]
      XCTAssertTrue(search.waitForExistence(timeout: 10), name)
      search.tap()
      guard app.keyboards.firstMatch.waitForExistence(timeout: 5) else {
        XCTFail("Search keyboard did not appear: \(name)")
        return
      }
      search.typeText("Metro Accessible Toilet")
      let row = app.buttons["place.row.21"]
      guard row.waitForExistence(timeout: 15) else {
        XCTFail("Search result did not appear: \(name)")
        return
      }
      attach(app, "metadata-\(name)-row")
      row.tap()
      XCTAssertTrue(app.staticTexts["place.title"].waitForExistence(timeout: 8), name)
      // Screenshot the top metadata (tags/hours) before scrolling.
      attach(app, "metadata-\(name)-detail-top")

      // At large sizes the actions stack vertically, so they cannot all be
      // visible at once. Expand the panel, then scroll to each one in turn and
      // confirm it is reachable, without requiring simultaneous visibility.
      if handle.exists, (handle.value as? String) != expandedValue { handle.tap() }
      let scroll = app.scrollViews["place.details"]
      guard scroll.waitForExistence(timeout: 5) else {
        XCTFail("Detail did not appear: \(name)")
        return
      }
      XCTAssertTrue(scrollTo(app, scroll, button: "place.share", name: name), name)
      XCTAssertTrue(scrollTo(app, scroll, button: "place.navigate", name: name), name)
      XCTAssertTrue(scrollTo(app, scroll, button: "place.bookmark", name: name), name)
      attach(app, "metadata-\(name)-detail-actions")
      app.terminate()
    }
  }

  // MARK: - Native venue Picker and serialized payload

  func testVenuePickerChangeCategoryAndSubmittedPayload() async throws {
    try await resetFixture()
    let app = launch(now: "2026-09-20T03:00:00Z")
    // Edit the metro fixture marker.
    let search = app.textFields["map.search"]
    XCTAssertTrue(search.waitForExistence(timeout: 8))
    search.tap()
    search.typeText("Metro Accessible Toilet")
    let row = app.buttons["place.row.21"]
    XCTAssertTrue(row.waitForExistence(timeout: 8))
    row.tap()
    XCTAssertTrue(app.buttons["place.edit"].waitForExistence(timeout: 8))
    app.buttons["place.edit"].tap()
    try signInIfNeeded(app)
    let picker = app.buttons["contribution.venue"]
    XCTAssertTrue(picker.waitForExistence(timeout: 10))
    attach(app, "metadata-editor-venue")
    // The existing tag is pre-filled.
    XCTAssertEqual(picker.value as? String, "Metro")

    // Really select a different venue and confirm it sticks.
    picker.tap()
    app.buttons["Mall"].tap()
    XCTAssertEqual(picker.value as? String, "Mall")

    // Switching to a non-toilet hides the Picker; back returns the other
    // default with no stale tag.
    let category = app.buttons["contribution.category"]
    category.tap()
    app.buttons["Nursing Rooms"].tap()
    XCTAssertFalse(app.buttons["contribution.venue"].exists)
    category.tap()
    app.buttons["Accessible Toilets"].tap()
    XCTAssertTrue(app.buttons["contribution.venue"].waitForExistence(timeout: 5))
    XCTAssertEqual(app.buttons["contribution.venue"].value as? String, "Other")

    // Choose a new venue and submit the proposal, then confirm the serialized
    // payload through the fixture's recorded writes.
    app.buttons["contribution.venue"].tap()
    app.buttons["Public toilet"].tap()
    attach(app, "metadata-editor-before-submit")
    app.buttons["contribution.submit"].tap()
    XCTAssertTrue(app.staticTexts["contribution.complete"].waitForExistence(timeout: 20))
    let write = try await waitForWrite(method: "PATCH", id: 21)
    XCTAssertEqual(write["venueType"] as? String, "public_toilet")
    // The fixture's published point is unchanged while the edit waits review.
    XCTAssertEqual(write["title"] as? String, "Metro Accessible Toilet")
  }

  func testNextDayToggleValidatesHoursAndRemovalClearsItsState() async throws {
    try await resetFixture()
    let app = launch(now: "2026-09-20T03:00:00Z")
    let search = app.textFields["map.search"]
    XCTAssertTrue(search.waitForExistence(timeout: 8))
    search.tap()
    search.typeText("Metro Accessible Toilet")
    let row = app.buttons["place.row.21"]
    XCTAssertTrue(row.waitForExistence(timeout: 8))
    row.tap()
    XCTAssertTrue(app.buttons["place.edit"].waitForExistence(timeout: 8))
    app.buttons["place.edit"].tap()
    try signInIfNeeded(app)

    // Make a real edit first, so the Submit state specifically reflects hours
    // validation instead of the unchanged-place guard.
    let venue = app.buttons["contribution.venue"]
    XCTAssertTrue(venue.waitForExistence(timeout: 10))
    venue.tap()
    app.buttons["Mall"].tap()
    let submit = app.buttons["contribution.submit"]
    XCTAssertTrue(submit.isEnabled)

    let nextDay = app.switches["contribution.next-day"]
    for _ in 0..<6 {
      if nextDay.isHittable { break }
      app.swipeUp()
    }
    XCTAssertTrue(nextDay.isHittable)
    XCTAssertEqual(nextDay.value as? String, "0")
    let preview = app.staticTexts["contribution.hours-preview"]
    XCTAssertEqual(preview.label, "09:00–22:00")
    let error = app.staticTexts["contribution.hours-error"]
    // SwiftUI exposes the complete labeled row as the Switch's AX frame.
    // Its midpoint can be blank space; press the native switch track itself.
    func toggleNextDay() {
      nextDay.coordinate(withNormalizedOffset: CGVector(dx: 0.92, dy: 0.5)).tap()
    }

    // 09:00 to next-day 22:00 exceeds the supported daily window.
    toggleNextDay()
    XCTAssertEqual(nextDay.value as? String, "1")
    XCTAssertFalse(submit.isEnabled)
    // Form creates its section footer lazily. At accessibility text sizes it
    // may be below the visible switch, so reveal it without tapping again.
    for _ in 0..<6 {
      if error.exists { break }
      app.swipeUp()
    }
    XCTAssertTrue(error.waitForExistence(timeout: 5))
    XCTAssertEqual(
      error.label,
      "For next-day closing, choose a time earlier than opening. Matching times mean open 24 hours.")
    XCTAssertFalse(preview.exists)
    attach(app, "metadata-next-day-invalid")

    for _ in 0..<6 {
      if nextDay.isHittable { break }
      app.swipeDown()
    }
    XCTAssertTrue(nextDay.isHittable)
    toggleNextDay()
    let restored = XCTNSPredicateExpectation(
      predicate: NSPredicate { _, _ in !error.exists && submit.isEnabled }, object: nil)
    let restoredResult = await XCTWaiter.fulfillment(of: [restored], timeout: 5)
    XCTAssertEqual(restoredResult, .completed)
    XCTAssertEqual(preview.label, "09:00–22:00")

    // Removing an invalid interval must clear the override as well as the two
    // times; adding hours again must start with a valid same-day selection.
    toggleNextDay()
    XCTAssertEqual(nextDay.value as? String, "1")
    XCTAssertFalse(submit.isEnabled)
    for _ in 0..<6 {
      if error.exists { break }
      app.swipeUp()
    }
    XCTAssertTrue(error.waitForExistence(timeout: 5))
    let remove = app.buttons["Remove opening hours"]
    for _ in 0..<4 {
      if remove.isHittable { break }
      app.swipeDown()
    }
    XCTAssertTrue(remove.isHittable)
    remove.tap()
    let add = app.buttons["Add opening hours"]
    XCTAssertTrue(add.waitForExistence(timeout: 5))
    XCTAssertFalse(nextDay.exists)
    XCTAssertFalse(error.exists)
    XCTAssertTrue(submit.isEnabled)
    add.tap()
    XCTAssertTrue(nextDay.waitForExistence(timeout: 5))
    XCTAssertEqual(nextDay.value as? String, "0")
    XCTAssertEqual(preview.label, "09:00–18:00")
    XCTAssertFalse(error.exists)
    XCTAssertTrue(submit.isEnabled)

    remove.tap()
    XCTAssertTrue(add.waitForExistence(timeout: 5))
    XCTAssertTrue(submit.isHittable)
    XCTAssertTrue(submit.isEnabled)
    attach(app, "metadata-next-day-hours-removed")
    submit.tap()
    XCTAssertTrue(app.staticTexts["contribution.complete"].waitForExistence(timeout: 20))
    let write = try await waitForWrite(method: "PATCH", id: 21)
    XCTAssertEqual(write["openTimeStart"] as? String, "")
    XCTAssertEqual(write["openTimeEnd"] as? String, "")
    XCTAssertEqual(write["venueType"] as? String, "mall")
    XCTAssertNil(write["closingDayOverride"])
    XCTAssertNil(write["closesNextDay"])
  }

  func testNewToiletOffersAllEightVenues() async throws {
    try await resetFixture()
    let app = launch(now: "2026-09-20T03:00:00Z")
    // Use the explicit map-selection entry above the new contribution form.
    try signInViaContribute(app)
    let useLocation = app.buttons["contribution.confirm-location"]
    XCTAssertTrue(useLocation.waitForExistence(timeout: 10))
    // Tap the map to choose a coordinate, then wait for the button to enable.
    app.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.4)).tap()
    let enabled = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "enabled == true"), object: useLocation)
    XCTAssertEqual(XCTWaiter.wait(for: [enabled], timeout: 5), .completed)
    useLocation.tap()
    let picker = app.buttons["contribution.venue"]
    XCTAssertTrue(picker.waitForExistence(timeout: 8))
    // A brand-new accessible toilet starts at the other default.
    XCTAssertEqual(picker.value as? String, "Other")
    picker.tap()
    for venue in [
      "Metro", "Hospital", "Mall", "Railway station", "School", "Airport", "Public toilet", "Other",
    ] {
      XCTAssertTrue(app.buttons[venue].waitForExistence(timeout: 3), venue)
    }
    app.buttons["Airport"].tap()
    XCTAssertEqual(picker.value as? String, "Airport")
    picker.tap()
    app.buttons["Public toilet"].tap()
    XCTAssertEqual(picker.value as? String, "Public toilet")
    attach(app, "metadata-new-venue")
  }

  func testCurrentLocationDraftCanChooseAnotherLocationWithoutLosingFields() async throws {
    try await resetFixture()
    let app = launch(now: "2026-09-20T03:00:00Z")
    try signInViaContribute(app, chooseOther: false)
    let title = app.textFields["contribution.title"]
    XCTAssertTrue(title.waitForExistence(timeout: 20), "The simulated GPS fix should create the draft")
    let changeLocation = app.buttons["contribution.location"]
    XCTAssertTrue(changeLocation.isHittable)
    XCTAssertLessThan(changeLocation.frame.minY, title.frame.minY)
    let original = try XCTUnwrap(changeLocation.value as? String)
    XCTAssertEqual(original, "31.23040, 121.47370", "Use the Core Location fix, not the viewport")
    fill(title, "Keep my contribution")
    title.typeText("\n")
    attach(app, "contribution-current-location")
    changeLocation.tap()
    let confirm = app.buttons["contribution.confirm-location"]
    XCTAssertTrue(confirm.waitForExistence(timeout: 8))
    XCTAssertEqual(confirm.value as? String, original)
    let alternate = app.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.35))
    alternate.tap()
    let changed = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "enabled == true AND value != %@", original), object: confirm)
    XCTAssertEqual(XCTWaiter.wait(for: [changed], timeout: 8), .completed)
    app.buttons["contribution.cancel-location"].tap()
    XCTAssertTrue(title.waitForExistence(timeout: 8))
    XCTAssertEqual(title.value as? String, "Keep my contribution")
    XCTAssertEqual(changeLocation.value as? String, original, "Cancel must preserve the saved point")

    changeLocation.tap()
    XCTAssertTrue(confirm.waitForExistence(timeout: 8))
    alternate.tap()
    let changedAgain = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "enabled == true AND value != %@", original), object: confirm)
    XCTAssertEqual(XCTWaiter.wait(for: [changedAgain], timeout: 8), .completed)
    let selected = try XCTUnwrap(confirm.value as? String)
    confirm.tap()
    XCTAssertTrue(title.waitForExistence(timeout: 8))
    XCTAssertEqual(title.value as? String, "Keep my contribution")
    XCTAssertEqual(changeLocation.value as? String, selected)
    attach(app, "contribution-alternate-location")
  }

  /// Run separately with simulator location permission denied. A synchronous
  /// denied callback must still dismiss the form and reach manual selection.
  func testDeniedLocationFallsBackToExplicitMapSelection() async throws {
    try await resetFixture()
    let app = launch(language: "zh", locale: true, now: "2026-09-20T03:00:00Z")
    try signInViaContribute(app, chooseOther: false)
    if app.textFields["contribution.title"].waitForExistence(timeout: 3) {
      throw XCTSkip("Run this case separately with simulator location permission denied")
    }
    let confirm = app.buttons["contribution.confirm-location"]
    XCTAssertTrue(confirm.waitForExistence(timeout: 10))
    XCTAssertFalse(confirm.isEnabled)
    XCTAssertEqual(confirm.value as? String, "")
    app.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.35)).tap()
    let selected = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "enabled == true AND value != ''"), object: confirm)
    XCTAssertEqual(XCTWaiter.wait(for: [selected], timeout: 8), .completed)
    let point = try XCTUnwrap(confirm.value as? String)
    confirm.tap()
    XCTAssertTrue(app.textFields["contribution.title"].waitForExistence(timeout: 8))
    XCTAssertEqual(app.buttons["contribution.location"].value as? String, point)
    attach(app, "contribution-denied-location-manual-draft")
  }

  // MARK: - Helpers

  private func launch(
    language: String = "en", large: Bool = false, locale: Bool = false, now: String
  ) -> XCUIApplication {
    let app = XCUIApplication()
    app.launchArguments = ["-AppleLanguages", "(\(language))"]
    app.launchArguments += ["-AppleLocale", locale ? "zh_Hans" : "en_US"]
    app.launchArguments += [
      "-lycoris-test-center", "31.2304,121.4737", "-lycoris.language", language,
      "-lycoris.searchType", "all", "-lycoris-test-metadata-now",
      String(Int(ISO8601DateFormatter().date(from: now)!.timeIntervalSince1970)),
    ]
    if large {
      app.launchArguments += [
        "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL",
      ]
    }
    app.launch()
    return app
  }

  /// Default creation uses GPS; choosing another point is an explicit action.
  private func signInViaContribute(_ app: XCUIApplication, chooseOther: Bool = true) throws {
    let contribute = app.buttons["map.contribute"]
    XCTAssertTrue(contribute.waitForExistence(timeout: 10))
    XCTAssertTrue(contribute.isHittable)
    contribute.tap()
    try signInIfNeeded(app)
    guard chooseOther else { return }
    let changeLocation = app.buttons["contribution.location"]
    let confirm = app.buttons["contribution.confirm-location"]
    let entry = XCTNSPredicateExpectation(
      predicate: NSPredicate { _, _ in changeLocation.exists || confirm.exists }, object: nil)
    XCTAssertEqual(XCTWaiter.wait(for: [entry], timeout: 10), .completed)
    if changeLocation.exists { changeLocation.tap() }
    XCTAssertTrue(confirm.waitForExistence(timeout: 10))
  }

  private func signInIfNeeded(_ app: XCUIApplication) throws {
    if app.textFields["auth.username"].waitForExistence(timeout: 8) {
      fill(app.textFields["auth.username"], fixtureAccount.username)
      fill(app.secureTextFields["auth.password"], fixtureAccount.password)
      app.secureTextFields["auth.password"].typeText("\n")
      try dismissPasswordSaveAlert(in: app)
    }
  }

  /// Scrolls until the identified button is hittable, or fails. Returns whether
  /// it became reachable within the attempt budget.
  private func scrollTo(
    _ app: XCUIApplication, _ scroll: XCUIElement, button identifier: String, name: String
  ) -> Bool {
    let element = app.buttons[identifier]
    for _ in 0..<14 {
      if element.isHittable { return true }
      scroll.swipeUp()
    }
    return element.isHittable
  }

  private func fill(_ element: XCUIElement, _ value: String) {
    element.tap()
    if let old = element.value as? String, old != element.placeholderValue, !old.isEmpty {
      element.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: old.count))
    }
    element.typeText(value)
  }

  private func attach(_ app: XCUIApplication, _ name: String) {
    let attachment = XCTAttachment(screenshot: app.screenshot())
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
  }

  private func waitForWrite(method: String, id: Int) async throws -> [String: Any] {
    for _ in 0..<40 {
      let writes = try await fixtureJSON(path: "__ui_fixture/writes") as? [[String: Any]] ?? []
      if let match = writes.first(where: {
        $0["method"] as? String == method && ($0["id"] as? Int) == id
      }), let body = match["body"] as? [String: Any] {
        return body
      }
      try await Task.sleep(for: .milliseconds(250))
    }
    XCTFail("No \(method) write for marker \(id) reached the fixture")
    throw WriteWaitError.missingWrite
  }

  private func resetFixture() async throws {
    try await requireFixture()
    var request = URLRequest(url: base.appendingPathComponent("__ui_fixture/reset"))
    request.httpMethod = "POST"
    let (_, response) = try await URLSession.shared.data(for: request)
    guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
      XCTFail("Fixture reset failed")
      throw WriteWaitError.missingWrite
    }
  }

  private func requireFixture() async throws {
    let value = try await fixtureJSON(path: "__ui_fixture")
    guard let fixture = value as? [String: Any],
      fixture["fixture"] as? String == "lycoris-place-metadata"
    else {
      throw XCTSkip("Start scripts/place-metadata-fixture.py on loopback first")
    }
  }

  /// Fetches fixture JSON with an async session and validates the HTTP status.
  private func fixtureJSON(path: String) async throws -> Any {
    let (data, response) = try await URLSession.shared.data(
      from: base.appendingPathComponent(path))
    guard let http = response as? HTTPURLResponse, (200..<300).contains(http.statusCode) else {
      throw WriteWaitError.missingWrite
    }
    return try JSONSerialization.jsonObject(with: data)
  }
}
