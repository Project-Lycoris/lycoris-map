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
      let app = launch(
        language: language, large: large, locale: locale, now: "2026-09-20T13:45:00Z")
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
    signInIfNeeded(app)
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

  func testNewToiletOffersAllEightVenues() async throws {
    try await resetFixture()
    let app = launch(now: "2026-09-20T03:00:00Z")
    signInViaContribute(app)
    app.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.4)).tap()
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

  func testMapSelectionPreservesFieldsAndSubmitsPickedCoordinate() async throws {
    try await resetFixture()
    let app = launch(language: "zh", locale: true, now: "2026-09-20T03:00:00Z")
    signInViaContribute(app)
    let title = app.textFields["contribution.title"]
    XCTAssertFalse(
      title.exists, "Entering contribution must not create a GPS or camera-center draft")
    XCTAssertFalse(app.buttons["contribution.confirm-location"].exists)
    let first = app.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.35))
    let second = app.coordinate(withNormalizedOffset: CGVector(dx: 0.65, dy: 0.5))
    first.press(forDuration: 0.05, thenDragTo: second)
    XCTAssertFalse(title.exists, "Panning is not selecting a point")
    first.tap()
    XCTAssertTrue(title.waitForExistence(timeout: 8), "A single map tap opens the form")
    let changeLocation = app.buttons["contribution.location"]
    let original = try XCTUnwrap(changeLocation.value as? String)
    XCTAssertFalse(original.isEmpty)
    fill(title, "手动选择的位置")
    title.typeText("\n")
    changeLocation.tap()
    XCTAssertTrue(app.staticTexts["contribution.pick-location"].waitForExistence(timeout: 8))
    app.buttons["contribution.cancel-location"].tap()
    XCTAssertTrue(title.waitForExistence(timeout: 8))
    XCTAssertEqual(title.value as? String, "手动选择的位置")
    XCTAssertEqual(changeLocation.value as? String, original)
    changeLocation.tap()
    XCTAssertTrue(app.staticTexts["contribution.pick-location"].waitForExistence(timeout: 8))
    second.tap()
    XCTAssertTrue(title.waitForExistence(timeout: 8))
    let selected = try XCTUnwrap(changeLocation.value as? String)
    XCTAssertNotEqual(selected, original)
    XCTAssertEqual(title.value as? String, "手动选择的位置")
    attach(app, "contribution-map-selected-form")
    app.buttons["contribution.submit"].tap()
    XCTAssertTrue(app.staticTexts["contribution.complete"].waitForExistence(timeout: 20))
    let write = try await waitForWrite(method: "POST")
    let lat = try XCTUnwrap(write["lat"] as? Double)
    let lng = try XCTUnwrap(write["lng"] as? Double)
    XCTAssertEqual(String(format: "%.5f, %.5f", lat, lng), selected)
  }

  func testCancellingMarkingModeDoesNotCreateADraft() async throws {
    try await resetFixture()
    let app = launch(language: "zh", locale: true, now: "2026-09-20T03:00:00Z")
    signInViaContribute(app)
    XCTAssertFalse(app.textFields["contribution.title"].exists)
    attach(app, "contribution-marking-mode")
    app.buttons["contribution.cancel-location"].tap()
    XCTAssertFalse(app.staticTexts["contribution.pick-location"].exists)
    XCTAssertFalse(app.textFields["contribution.title"].exists)
    app.buttons["map.contribute"].tap()
    XCTAssertTrue(app.staticTexts["contribution.pick-location"].waitForExistence(timeout: 8))
    XCTAssertFalse(app.textFields["contribution.title"].exists)
    app.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.35)).tap()
    XCTAssertTrue(app.textFields["contribution.title"].waitForExistence(timeout: 8))
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

  private func signInViaContribute(_ app: XCUIApplication) {
    let contribute = app.buttons["map.contribute"]
    XCTAssertTrue(contribute.waitForExistence(timeout: 10))
    XCTAssertTrue(contribute.isHittable)
    contribute.tap()
    signInIfNeeded(app)
    XCTAssertTrue(app.staticTexts["contribution.pick-location"].waitForExistence(timeout: 10))
  }

  private func signInIfNeeded(_ app: XCUIApplication) {
    if app.textFields["auth.username"].waitForExistence(timeout: 8) {
      fill(app.textFields["auth.username"], fixtureAccount.username)
      fill(app.secureTextFields["auth.password"], fixtureAccount.password)
      app.secureTextFields["auth.password"].typeText("\n")
      declinePasswordSave(app)
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

  private func declinePasswordSave(_ app: XCUIApplication) {
    for host in [app, XCUIApplication(bundleIdentifier: "com.apple.springboard")] {
      let button = host.buttons.matching(
        NSPredicate(format: "label IN %@", ["Not Now", "以后", "以后再说"])
      ).firstMatch
      if button.waitForExistence(timeout: 3) {
        button.tap()
        return
      }
    }
  }

  private func attach(_ app: XCUIApplication, _ name: String) {
    let attachment = XCTAttachment(screenshot: app.screenshot())
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
  }

  private func waitForWrite(method: String, id: Int? = nil) async throws -> [String: Any] {
    for _ in 0..<40 {
      let writes = try await fixtureJSON(path: "__ui_fixture/writes") as? [[String: Any]] ?? []
      if let match = writes.first(where: {
        $0["method"] as? String == method && (id == nil || ($0["id"] as? Int) == id)
      }), let body = match["body"] as? [String: Any] {
        return body
      }
      try await Task.sleep(for: .milliseconds(250))
    }
    XCTFail("No \(method) write for marker \(id.map(String.init) ?? "new") reached the fixture")
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
