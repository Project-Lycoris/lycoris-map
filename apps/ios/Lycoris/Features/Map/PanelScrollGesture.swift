import SwiftUI
import UIKit

/// One continuous vertical travel budget: expand before scrolling up; consume
/// the existing scroll offset before pulling the panel down. Translation uses
/// the window, so moving/resizing the panel cannot feed back into the gesture.
struct PanelScrollTravel {
  let expandedTop: CGFloat
  let collapsedTop: CGFloat
  private(set) var initialTop: CGFloat
  private(set) var initialScrollOffset: CGFloat
  private var previousTranslation: CGFloat = 0

  init(expandedTop: CGFloat, collapsedTop: CGFloat, initialTop: CGFloat, initialScrollOffset: CGFloat) {
    self.expandedTop = expandedTop
    self.collapsedTop = collapsedTop
    self.initialTop = initialTop
    self.initialScrollOffset = initialScrollOffset
  }

  struct Position: Equatable {
    let top: CGFloat
    let scrollOffset: CGFloat
  }

  func position(translation: CGFloat) -> Position {
    let expansion = max(0, initialTop - expandedTop)
    let scroll = expansion > 1 ? 0 : max(0, initialScrollOffset)
    let remaining = scroll - expansion - translation
    return Position(
      top: min(collapsedTop, expandedTop + max(0, -remaining)),
      scrollOffset: max(0, remaining))
  }

  /// Discard travel beyond either end, so reversing responds immediately even
  /// when the content is shorter than the fully expanded panel.
  mutating func advance(translation: CGFloat, maximumScrollOffset: CGFloat) -> Position {
    let next = position(translation: translation - previousTranslation)
    initialTop = next.top
    initialScrollOffset = min(max(0, maximumScrollOffset), next.scrollOffset)
    previousTranslation = translation
    return Position(top: initialTop, scrollOffset: initialScrollOffset)
  }
}

/// Cooperates with the actual List/ScrollView under the finger. Gestures that
/// only scroll remain native. Once the panel moves, its travel and the content
/// offset are coordinated until release, then UIKit resumes deceleration.
struct PanelScrollGesture: UIGestureRecognizerRepresentable {
  let layout: PanelLayout
  let restingTop: CGFloat
  let isExpanded: Bool
  let generation: Int
  var onMove: (CGFloat) -> Void
  var onEnd: (CGFloat, CGFloat) -> Void

  func makeCoordinator(converter: CoordinateSpaceConverter) -> Coordinator {
    Coordinator(self)
  }

  func makeUIGestureRecognizer(context: Context) -> UIPanGestureRecognizer {
    let pan = UIPanGestureRecognizer()
    pan.maximumNumberOfTouches = 1
    pan.delegate = context.coordinator
    return pan
  }

  func updateUIGestureRecognizer(_ recognizer: UIPanGestureRecognizer, context: Context) {
    if context.coordinator.owner.generation != generation {
      context.coordinator.cancel(recognizer)
    }
    context.coordinator.owner = self
  }

  func handleUIGestureRecognizerAction(_ recognizer: UIPanGestureRecognizer, context: Context) {
    context.coordinator.handle(recognizer)
  }

  @MainActor final class Coordinator: NSObject, UIGestureRecognizerDelegate {
    var owner: PanelScrollGesture
    private weak var scrollView: UIScrollView?
    private var travel: PanelScrollTravel?
    private var movedPanel = false
    private var pinnedOffset: CGFloat?
    private var observation: NSKeyValueObservation?
    private var adjustingOffset = false
    private var initialScrollOffset: CGFloat = 0

    init(_ owner: PanelScrollGesture) { self.owner = owner }

    func gestureRecognizer(_ gestureRecognizer: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
      guard travel == nil else { return false }
      scrollView = nil
      initialScrollOffset = 0
      var view = touch.view
      while let current = view {
        if let scroll = current as? UIScrollView, scroll.isScrollEnabled {
          // A text editor owns its selection/scrolling; the surrounding panel
          // remains draggable from all other content and empty areas.
          if scroll is UITextView { return false }
          scrollView = scroll
          initialScrollOffset = max(0, scroll.contentOffset.y + scroll.adjustedContentInset.top)
          break
        }
        if current === gestureRecognizer.view { break }
        view = current.superview
      }
      return true
    }

    func gestureRecognizerShouldBegin(_ gestureRecognizer: UIGestureRecognizer) -> Bool {
      guard let pan = gestureRecognizer as? UIPanGestureRecognizer else { return false }
      let velocity = pan.velocity(in: pan.view?.window)
      return abs(velocity.y) > abs(velocity.x)
    }

    func gestureRecognizer(
      _ gestureRecognizer: UIGestureRecognizer,
      shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
    ) -> Bool {
      otherGestureRecognizer === scrollView?.panGestureRecognizer
    }

    func handle(_ pan: UIPanGestureRecognizer) {
      switch pan.state {
      case .began:
        movedPanel = false
        pinnedOffset = nil
        travel = PanelScrollTravel(
          expandedTop: owner.layout.expandedTop, collapsedTop: owner.layout.collapsedTop,
          initialTop: owner.restingTop, initialScrollOffset: initialScrollOffset)
        // List can update its offset during layout, after our pan callback.
        // Keep the consumed portion pinned across those native updates as well.
        observation = scrollView?.observe(\.contentOffset, options: [.new]) { [weak self] _, _ in
          MainActor.assumeIsolated { self?.applyPinnedOffset() }
        }
        update(pan)
      case .changed:
        update(pan)
      case .ended, .cancelled, .failed:
        guard self.travel != nil else { return }
        update(pan)
        guard let travel else { return }
        let position = travel.position(translation: 0)
        observation = nil
        pinnedOffset = nil
        if movedPanel {
          if position.top > travel.expandedTop + 1, let scrollView {
            // A fling used to move the panel must not start an invisible list
            // deceleration while the panel is settling at a smaller detent.
            scrollView.panGestureRecognizer.isEnabled = false
            scrollView.panGestureRecognizer.isEnabled = true
            scrollView.setContentOffset(CGPoint(x: scrollView.contentOffset.x,
              y: -scrollView.adjustedContentInset.top), animated: false)
          }
          let velocity = pan.state == .ended ? pan.velocity(in: pan.view?.window).y : 0
          owner.onEnd(position.top, position.scrollOffset > 0 ? 0 : velocity)
        }
        self.travel = nil
        movedPanel = false
      default: break
      }
    }

    private func update(_ pan: UIPanGestureRecognizer) {
      guard var travel else { return }
      let maximumOffset = scrollView.map {
        max(0, $0.contentSize.height - $0.bounds.height
          + $0.adjustedContentInset.top + $0.adjustedContentInset.bottom)
      } ?? 0
      // Until we resize the panel, UIKit owns bottom bounce as well. Do not
      // discard its overscroll travel and then collapse before it reaches top.
      let nativeBounce = scrollView.map {
        $0.bounces && (maximumOffset > 0 || $0.alwaysBounceVertical)
      } ?? false
      let nativeScroll = !movedPanel && nativeBounce && owner.isExpanded
      let translation = pan.translation(in: pan.view?.window).y
      let position = travel.advance(
        translation: translation,
        maximumScrollOffset: nativeScroll ? .greatestFiniteMagnitude : maximumOffset)
      self.travel = travel
      if abs(position.top - owner.restingTop) > 0.5 { movedPanel = true }
      // Nearby and expanded can share a coordinate at accessibility text sizes.
      // An upward drag must still reveal full content and settle as expanded.
      if !owner.isExpanded && translation < 0 && position.top <= travel.expandedTop + 1 {
        movedPanel = true
      }
      guard movedPanel else { return }
      pinnedOffset = position.scrollOffset
      applyPinnedOffset()
      owner.onMove(position.top)
    }

    func cancel(_ pan: UIPanGestureRecognizer) {
      let cancelScroll = movedPanel && (pan.state == .began || pan.state == .changed)
      observation = nil
      pinnedOffset = nil
      travel = nil
      movedPanel = false
      pan.isEnabled = false
      pan.isEnabled = true
      if cancelScroll, let scrollView {
        scrollView.panGestureRecognizer.isEnabled = false
        scrollView.panGestureRecognizer.isEnabled = true
      }
    }

    private func applyPinnedOffset() {
      guard !adjustingOffset, let scrollView, let pinnedOffset else { return }
      let minimum = -scrollView.adjustedContentInset.top
      let maximum = max(minimum, scrollView.contentSize.height - scrollView.bounds.height
        + scrollView.adjustedContentInset.bottom)
      let target = min(maximum, minimum + pinnedOffset)
      guard abs(scrollView.contentOffset.y - target) > 0.5 else { return }
      adjustingOffset = true
      scrollView.contentOffset.y = target
      adjustingOffset = false
    }
  }
}
