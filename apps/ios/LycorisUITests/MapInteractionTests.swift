import XCTest

@MainActor
final class MapInteractionTests: LocalBackendTestCase {
  func testOverlappingNearbyDetentStillExpandsFromContent() {
    let app = XCUIApplication()
    app.launchArguments = ["-AppleLanguages", "(en)", "-lycoris-preview", "collapsed",
      "-lycoris.language", "en",
      "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
    app.launch()
    let handle = app.buttons["map.panel.handle"]
    XCTAssertTrue(handle.waitForExistence(timeout: 10))
    handle.tap()
    let nearby = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "value == %@", "Nearby"), object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [nearby], timeout: 4), .completed)
    XCTAssertLessThan(handle.frame.minY, 100, "Nearby must coincide with the expanded top in this fixture")
    let category = app.buttons["map.category.accessible_toilet"]
    XCTAssertTrue(category.waitForExistence(timeout: 3))
    category.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
      .press(forDuration: 0.1,
        thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.12)))
    let expanded = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "value == %@", "Expanded"), object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [expanded], timeout: 4), .completed)
    let range = app.buttons["settings.range"]
    for _ in 0..<6 {
      if range.isHittable { break }
      app.descendants(matching: .any)["map.panel.content"].firstMatch.swipeUp()
    }
    XCTAssertTrue(range.isHittable)
  }

  func testContentRowsResizeThePanelAndStillAcceptTaps() {
    let app = XCUIApplication()
    app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US",
      "-lycoris-preview", "anonymousExpanded", "-lycoris.language", "en"]
    app.launch()
    let handle = app.buttons["map.panel.handle"]
    let category = app.buttons["map.category.accessible_toilet"]
    XCTAssertTrue(category.waitForExistence(timeout: 10))
    let originalTop = handle.frame.minY
    category.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
      .press(forDuration: 0.1,
        thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.92)))
    let lowered = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
      handle.value as? String != "Expanded" && handle.frame.minY > originalTop + 100
    }, object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [lowered], timeout: 4), .completed)
    XCTAssertFalse(app.keyboards.firstMatch.exists)
    if handle.value as? String == "Collapsed" { handle.tap() }
    XCTAssertTrue(category.waitForExistence(timeout: 3))
    category.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
      .press(forDuration: 0.1,
        thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.4, dy: 0.12)))
    let expanded = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "value == %@", "Expanded"), object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [expanded], timeout: 4), .completed)
    XCTAssertFalse(app.alerts.firstMatch.exists)
    XCTAssertFalse(app.staticTexts["places.results.title"].exists)
    attach(app, name: "expanded-from-content-row")
    // A drag that begins over a button must not trigger it, but an ordinary
    // tap must still open the native settings screen.
    let range = app.buttons["settings.range"]
    XCTAssertTrue(range.isHittable)
    range.tap()
    XCTAssertTrue(app.navigationBars["Searching Range"].waitForExistence(timeout: 3))
  }

  func testScrolledDetailsReachTopBeforeThePanelCollapses() {
    let app = XCUIApplication()
    app.launchArguments = ["-AppleLanguages", "(en)", "-lycoris-preview", "details",
      "-lycoris.language", "en",
      "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"]
    app.launch()
    let handle = app.buttons["map.panel.handle"]
    XCTAssertTrue(handle.waitForExistence(timeout: 10))
    handle.tap()
    let scroll = app.scrollViews["place.details"]
    XCTAssertTrue(scroll.waitForExistence(timeout: 3))
    for _ in 0..<3 { scroll.swipeUp() }
    XCTAssertEqual(handle.value as? String, "Expanded")
    XCTAssertTrue(app.buttons["place.bookmark"].isHittable)
    let expandedTop = handle.frame.minY
    let bookmarkTop = app.buttons["place.bookmark"].frame.minY
    scroll.coordinate(withNormalizedOffset: CGVector(dx: 0.45, dy: 0.45))
      .press(forDuration: 0.1,
        thenDragTo: scroll.coordinate(withNormalizedOffset: CGVector(dx: 0.45, dy: 0.58)))
    XCTAssertEqual(handle.value as? String, "Expanded")
    XCTAssertEqual(handle.frame.minY, expandedTop, accuracy: 1)
    XCTAssertGreaterThan(app.buttons["place.bookmark"].frame.minY, bookmarkTop + 20)
    // Return through the scrollable content. Once its top is reached, the same
    // content surface must move the panel; no grabber/search-row interaction.
    for _ in 0..<8 {
      if handle.value as? String != "Expanded" { break }
      scroll.coordinate(withNormalizedOffset: CGVector(dx: 0.45, dy: 0.25))
        .press(forDuration: 0.1,
          thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.45, dy: 0.93)))
    }
    let lowered = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "value != %@", "Expanded"), object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [lowered], timeout: 4), .completed)
    attach(app, name: "detail-scroll-to-panel-handoff")
  }

  func testCollapsedSearchRowAndPaddingDragWithoutStealingTaps() {
    let app = XCUIApplication()
    app.launchArguments = [
      "-AppleLanguages", "(en)", "-AppleLocale", "en_US",
      "-lycoris-preview", "collapsed", "-lycoris.language", "en",
    ]
    app.launch()
    let handle = app.buttons["map.panel.handle"]
    let search = app.textFields["map.search"]
    XCTAssertTrue(handle.waitForExistence(timeout: 10))
    let collapsedTop = handle.frame.minY
    attach(app, name: "liquid-glass-collapsed")
    for fromPadding in [false, true] {
      XCTAssertEqual(handle.value as? String, "Collapsed")
      let start =
        fromPadding
        ? app.buttons["map.account"].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 1.15))
        : search.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
      start.press(
        forDuration: 0.1,
        thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.12)))
      let expanded = XCTNSPredicateExpectation(
        predicate: NSPredicate(format: "value == %@", "Expanded"), object: handle)
      XCTAssertEqual(XCTWaiter.wait(for: [expanded], timeout: 4), .completed)
      XCTAssertFalse(app.keyboards.firstMatch.exists)
      attach(app, name: fromPadding ? "panel-drag-from-padding" : "panel-drag-from-search")
      handle.tap()
      // A detent value changes before its spring finishes. Wait for the actual
      // collapsed position before tapping the moving account/search controls.
      let settled = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
        handle.value as? String == "Collapsed" && abs(handle.frame.minY - collapsedTop) < 1
      }, object: handle)
      XCTAssertEqual(XCTWaiter.wait(for: [settled], timeout: 4), .completed)
    }
    app.buttons["map.account"].tap()
    XCTAssertTrue(app.alerts["Not available yet"].waitForExistence(timeout: 3))
    app.alerts.buttons["OK"].tap()
    search.tap()
    XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
    search.typeText("library")
    XCTAssertEqual(search.value as? String, "library")
  }

  func testAccessibilityTextKeepsDetailActionsReachable() {
    let app = XCUIApplication()
    app.launchArguments = [
      "-AppleLanguages", "(en)", "-lycoris-preview", "details",
      "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL",
    ]
    app.launchArguments += ["-lycoris.language", "en"]
    app.launch()
    let title = app.staticTexts["place.title"]
    XCTAssertTrue(title.waitForExistence(timeout: 10))
    XCTAssertGreaterThan(title.frame.height, 150)
    let handle = app.buttons["map.panel.handle"]
    handle.tap()
    let expanded = XCTNSPredicateExpectation(
      predicate: NSPredicate { object, _ in
        guard let element = object as? XCUIElement else { return false }
        return element.value as? String == "Expanded" && element.frame.minY < 100
      }, object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [expanded], timeout: 3), .completed)
    let scroll = app.scrollViews["place.details"]
    for _ in 0..<6 {
      if app.buttons["Bookmark place"].isHittable { break }
      scroll.swipeUp()
    }
    XCTAssertTrue(app.buttons["Share"].isHittable)
    XCTAssertTrue(app.buttons["Navigate"].isHittable)
    XCTAssertTrue(app.buttons["Bookmark place"].isHittable)
    attach(app, name: "10-accessibility-detail-actions")
  }

  func testBookmarkPreviewOpensAndDismissesDetails() throws {
    let app = XCUIApplication()
    app.launchArguments = [
      "-AppleLanguages", "(en)", "-AppleLocale", "en_US", "-lycoris-preview", "expanded",
    ]
    app.launchArguments += ["-lycoris.language", "en"]
    app.launch()
    let heading = app.buttons["map.bookmarks.heading"]
    XCTAssertTrue(heading.waitForExistence(timeout: 10))
    attach(app, name: "07-expanded-bookmarks")
    app.buttons["place.row.figma-preview-1"].tap()
    let title = app.staticTexts["place.title"]
    XCTAssertTrue(title.waitForExistence(timeout: 3))
    XCTAssertTrue(app.buttons["Share"].isHittable)
    XCTAssertTrue(app.buttons["Navigate"].isHittable)
    XCTAssertTrue(app.buttons["Bookmark place"].isHittable)
    // A partially clipped button may still be hittable. The resting detail
    // panel must fit its photo and complete action row above the screen edge.
    for action in ["place.share", "place.navigate", "place.bookmark"] {
      XCTAssertLessThanOrEqual(app.buttons[action].frame.maxY, app.frame.maxY, action)
    }
    attach(app, name: "08-place-details")

    app.buttons["Navigate"].tap()
    XCTAssertTrue(app.alerts["Not available yet"].waitForExistence(timeout: 3))
    app.alerts.buttons["OK"].tap()
    let handle = app.buttons["map.panel.handle"]
    handle.tap()
    let expanded = XCTNSPredicateExpectation(
      predicate: NSPredicate { object, _ in
        guard let element = object as? XCUIElement else { return false }
        return element.value as? String == "Expanded" && element.frame.minY < 100
      }, object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [expanded], timeout: 3), .completed)
    attach(app, name: "08a-details-expanded")
    XCTAssertTrue(title.exists)
    handle.tap()
    let collapsed = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "value == %@", "Collapsed"), object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [collapsed], timeout: 3), .completed)
    XCTAssertFalse(title.exists)
    XCTAssertTrue(app.textFields["map.search"].exists)
    app.buttons["map.nearby"].tap()
    handle.tap()
    XCTAssertTrue(heading.exists)

    app.terminate()
    app.launchArguments = ["-AppleLanguages", "(en)", "-lycoris-preview", "anonymousExpanded"]
    app.launchArguments += ["-lycoris.language", "en"]
    app.launch()
    XCTAssertTrue(app.staticTexts["Settings"].waitForExistence(timeout: 10))
    XCTAssertFalse(app.buttons["map.bookmarks.heading"].exists)
    XCTAssertFalse(app.buttons["place.row.figma-preview-1"].exists)
    attach(app, name: "09-anonymous-expanded")
  }

  func testPanelDragKeyboardAndMapInteraction() throws {
    let app = XCUIApplication()
    app.launchArguments = [
      "-AppleLanguages", "(en)", "-AppleLocale", "en_US", "-lycoris-preview", "collapsed",
    ]
    app.launchArguments += ["-lycoris.language", "en"]
    app.launch()
    let handle = app.buttons["map.panel.handle"]
    XCTAssertTrue(handle.waitForExistence(timeout: 10))
    XCTAssertEqual(handle.value as? String, "Collapsed")
    attach(app, name: "01-collapsed")

    app.buttons["map.nearby"].tap()
    XCTAssertTrue(NSPredicate(format: "value == %@", "Nearby").evaluate(with: handle))
    let mapStart = app.coordinate(withNormalizedOffset: CGVector(dx: 0.35, dy: 0.32))
    let mapEnd = app.coordinate(withNormalizedOffset: CGVector(dx: 0.68, dy: 0.50))
    mapStart.press(forDuration: 0.15, thenDragTo: mapEnd)
    XCTAssertEqual(handle.value as? String, "Nearby")
    attach(app, name: "02-map-panned-with-panel")

    handle.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
      .press(
        forDuration: 0.15,
        thenDragTo: app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.10)))
    let expanded = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "value == %@", "Expanded"), object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [expanded], timeout: 3), .completed)
    XCTAssertEqual(handle.frame.minX, app.frame.minX, accuracy: 1)
    XCTAssertEqual(handle.frame.maxX, app.frame.maxX, accuracy: 1)
    attach(app, name: "03-expanded")

    let search = app.textFields["map.search"]
    search.tap()
    search.typeText("library")
    XCTAssertEqual(search.value as? String, "library")
    XCTAssertTrue(search.isHittable)
    attach(app, name: "04-search-keyboard")
    let keyboard = app.keyboards.firstMatch
    XCTAssertTrue(keyboard.waitForExistence(timeout: 3))
    let start = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.48))
    let end = app.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.24))
    start.press(forDuration: 0.1, thenDragTo: end)
    let about = app.buttons["settings.about"]
    XCTAssertTrue(about.isHittable)
    XCTAssertLessThan(about.frame.maxY, keyboard.frame.minY)
    attach(app, name: "05-content-scroll-with-keyboard")
    handle.tap()
    let collapsed = XCTNSPredicateExpectation(
      predicate: NSPredicate(format: "value == %@", "Collapsed"), object: handle)
    XCTAssertEqual(XCTWaiter.wait(for: [collapsed], timeout: 3), .completed)
    XCTAssertEqual(search.value as? String, "library")
    attach(app, name: "06-collapsed-preserves-input")
  }

  private func attach(_ app: XCUIApplication, name: String) {
    let attachment = XCTAttachment(screenshot: app.screenshot())
    attachment.name = name
    attachment.lifetime = .keepAlways
    add(attachment)
  }
}
