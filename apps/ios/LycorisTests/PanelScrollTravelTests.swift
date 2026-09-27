import Foundation
import Testing

@testable import Lycoris

struct PanelScrollTravelTests {
  @Test func reversalAtBothBoundsHasNoDeadTravel() {
    var travel = PanelScrollTravel(expandedTop: 70, collapsedTop: 780,
      initialTop: 500, initialScrollOffset: 0)
    #expect(travel.advance(translation: -900, maximumScrollOffset: 0)
      == .init(top: 70, scrollOffset: 0))
    #expect(travel.advance(translation: -880, maximumScrollOffset: 0)
      == .init(top: 90, scrollOffset: 0))
    #expect(travel.advance(translation: 900, maximumScrollOffset: 0)
      == .init(top: 780, scrollOffset: 0))
    #expect(travel.advance(translation: 880, maximumScrollOffset: 0)
      == .init(top: 760, scrollOffset: 0))
  }

  @Test func reversalAtScrollableContentBottomStartsImmediately() {
    var travel = PanelScrollTravel(expandedTop: 70, collapsedTop: 780,
      initialTop: 500, initialScrollOffset: 0)
    #expect(travel.advance(translation: -900, maximumScrollOffset: 120)
      == .init(top: 70, scrollOffset: 120))
    #expect(travel.advance(translation: -880, maximumScrollOffset: 120)
      == .init(top: 70, scrollOffset: 100))
    #expect(travel.advance(translation: -730, maximumScrollOffset: 120)
      == .init(top: 120, scrollOffset: 0))
  }
  @Test func oneUpwardGestureExpandsBeforeScrolling() {
    let travel = PanelScrollTravel(expandedTop: 70, collapsedTop: 780,
      initialTop: 500, initialScrollOffset: 0)
    #expect(travel.position(translation: -200) == .init(top: 300, scrollOffset: 0))
    #expect(travel.position(translation: -430) == .init(top: 70, scrollOffset: 0))
    #expect(travel.position(translation: -530) == .init(top: 70, scrollOffset: 100))
  }

  @Test func oneDownwardGestureScrollsToTopBeforeCollapsing() {
    let travel = PanelScrollTravel(expandedTop: 70, collapsedTop: 780,
      initialTop: 70, initialScrollOffset: 180)
    #expect(travel.position(translation: 100) == .init(top: 70, scrollOffset: 80))
    #expect(travel.position(translation: 180) == .init(top: 70, scrollOffset: 0))
    #expect(travel.position(translation: 260) == .init(top: 150, scrollOffset: 0))
    #expect(travel.position(translation: 1_000) == .init(top: 780, scrollOffset: 0))
  }

  @Test func reversalDoesNotJumpAtTheScrollBoundary() {
    let travel = PanelScrollTravel(expandedTop: 70, collapsedTop: 780,
      initialTop: 500, initialScrollOffset: 0)
    for translation in [-450.0, -430, -400, -430, -450] {
      let result = travel.position(translation: translation)
      #expect(result.top >= 70 && result.scrollOffset >= 0)
      #expect(result.top == 70 || result.scrollOffset == 0)
    }
    #expect(travel.position(translation: 0) == .init(top: 500, scrollOffset: 0))
  }

  @Test func partialPanelDoesNotInheritAnOldExpandedScrollOffset() {
    let travel = PanelScrollTravel(expandedTop: 70, collapsedTop: 780,
      initialTop: 500, initialScrollOffset: 200)
    #expect(travel.position(translation: 20) == .init(top: 520, scrollOffset: 0))
  }
}
