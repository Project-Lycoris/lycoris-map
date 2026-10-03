import Foundation
import MapKit
import Testing

@testable import Lycoris

struct PlaceDataTests {
  @Test func decodesRustInt64AndOptionalFieldsWithoutPrecisionLoss() throws {
    let marker = try JSONDecoder().decode(
      Marker.self,
      from: Data(
        #"{"id":9223372036854775806,"version":9,"lat":31.2304,"lng":121.4737,"category":"accessible_toilet","title":"上海","description":null,"openTimeStart":null,"openTimeEnd":null,"markImage":null,"contentLanguage":"zh"}"#
          .utf8))
    #expect(marker.id == 9_223_372_036_854_775_806)
    #expect(marker.description == nil)
    #expect(marker.contentLanguage == "zh")
    #expect(marker.point?.latitude == 31.2304)
  }

  @Test func requestContractEncodesQueriesAndUsesExactRustNames() throws {
    let api = MarkerAPI(baseURL: URL(string: "https://example.test"))
    let request = try api.request(
      for: .viewport(MarkerBounds(south: 30, west: 120, north: 32, east: 122)), language: "en")
    let url = try #require(request.url)
    let components = try #require(URLComponents(url: url, resolvingAgainstBaseURL: false))
    let params = Dictionary(
      uniqueKeysWithValues: components.queryItems!.map { ($0.name, $0.value!) })
    #expect(url.path == "/api/markers/viewport")
    #expect(
      params == [
        "minLat": "30.0", "maxLat": "32.0", "minLng": "120.0", "maxLng": "122.0", "lang": "en",
      ])
    let search = try api.request(for: .search("  A & 上海 + 1  "), language: "zh")
    #expect(
      URLComponents(url: search.url!, resolvingAgainstBaseURL: false)?.queryItems?.first(where: {
        $0.name == "q"
      })?.value == "A & 上海 + 1")
    let nearby = try api.request(
      for: .nearby(GeoPoint(latitude: 31, longitude: 121)!, .nursing), language: "en")
    #expect(nearby.url?.query?.contains("category=baby_room") == true)
    #expect(nearby.url?.query?.contains("radius=1000") == true)
    #expect(api.session.configuration.httpCookieStorage == nil)
    #expect(api.session.configuration.httpShouldSetCookies == false)
  }

  @Test func hoursAndImagesDoNotInventContent() {
    #expect(
      PlacePresentation.hours(start: nil, end: nil) == String(appLocalized: "Hours not provided"))
    #expect(
      PlacePresentation.hours(start: "25:00", end: "09:00")
        == String(appLocalized: "Hours not provided"))
    #expect(
      PlacePresentation.hours(start: "00:00", end: "00:00") == String(appLocalized: "Open 24 hours")
    )
    #expect(
      PlacePresentation.hours(start: "21:00", end: "06:00")
        == String(
          format: String(appLocalized: "%@–%@ (next day)", table: "OpeningHours"),
          "21:00", "06:00"))
    let base = URL(string: "https://api.example.test")!
    #expect(
      PlacePresentation.imageURL("/uploads/markers/abc-1.png", baseURL: base)?.host == base.host)
    for path in [
      "https://other.test/a.jpg", "/uploads/markers/..", "/uploads/markers/%2e%2e",
      "/uploads/markers/a.png?token=x",
    ] {
      #expect(PlacePresentation.imageURL(path, baseURL: base) == nil)
    }
  }

  @Test(arguments: [(31.2304, 121.4737), (40.766, -74.077)])
  func coordinatesRoundTripInChinaAndOutsideChina(point: (Double, Double)) throws {
    let original = try #require(GeoPoint(latitude: point.0, longitude: point.1))
    let projected = MKMapPoint(original.coordinate).coordinate
    #expect(abs(projected.latitude - original.latitude) < 0.0000001)
    #expect(abs(projected.longitude - original.longitude) < 0.0000001)
    let north = GeoPoint(latitude: point.0 + 0.001, longitude: point.1)!
    #expect((110...112).contains(original.distance(to: north)))
    #expect(original.distance(to: original) == 0)
  }

  @Test func dateLineAndWorldViewportsSplitIntoBackendCompatibleBounds() throws {
    let world = MKMapRect.world
    let center = CLLocationCoordinate2D(latitude: 0, longitude: 179)
    let rect = MKMapRect(
      x: world.width * 0.99, y: world.height * 0.45, width: world.width * 0.02,
      height: world.height * 0.1)
    let viewport = try #require(MapViewport(rect: rect, center: center))
    #expect(viewport.bounds.count == 2)
    #expect(viewport.bounds[0].west > 170)
    #expect(viewport.bounds[0].east == 180)
    #expect(viewport.bounds[1].west == -180)
    #expect(viewport.bounds[1].east < -170)
    #expect(viewport.bounds.allSatisfy { $0.west < $0.east && $0.south < $0.north })
    let wide = try #require(MapViewport(rect: world, center: center))
    #expect(wide.bounds.count == 1)
    #expect(wide.bounds[0].west == -180 && wide.bounds[0].east == 180)
    let westWrapped = try #require(
      MapViewport(
        rect: MKMapRect(
          x: -world.width * 0.01, y: rect.minY, width: rect.width, height: rect.height),
        center: center))
    #expect(westWrapped.bounds == viewport.bounds)
    #expect(GeoPoint(latitude: .nan, longitude: 0) == nil)
    #expect(GeoPoint(latitude: 0, longitude: 181) == nil)
  }

  @Test func shareIncludesOnlyPublicDestination() {
    let place = PlacePresentation(
      marker: sampleMarker(1), origin: GeoPoint(latitude: 30, longitude: 120), located: true,
      baseURL: nil)
    let components = URLComponents(url: place.shareURL!, resolvingAgainstBaseURL: false)!
    let params = Dictionary(
      uniqueKeysWithValues: components.queryItems!.map { ($0.name, $0.value!) })
    #expect(params == ["markerId": "1"])
    #expect(components.scheme == "lycoris" && components.host == "maps")
    #expect(place.photoAsset == nil)
  }
}

func sampleMarker(_ id: Int64, category: PlaceCategory = .toilet) -> Marker {
  Marker(
    id: id, version: 1, lat: 31.2304, lng: 121.4737, category: category,
    title: "Place \(id)", description: nil, openTimeStart: nil, openTimeEnd: nil,
    markImage: nil, contentLanguage: "en")
}

actor DeferredMarkers: MarkerServing {
  nonisolated let baseURL: URL? = nil
  enum Request: Equatable {
    case list(MarkerQuery)
    case detail(Int64)
  }
  private(set) var requests: [Request] = []
  private var pending: [Int: CheckedContinuation<[Marker], any Error>] = [:]

  func markers(_ query: MarkerQuery, language: String) async throws -> [Marker] {
    try await enqueue(.list(query))
  }
  func detail(id: Int64, language: String) async throws -> Marker {
    try await enqueue(.detail(id))[0]
  }
  private func enqueue(_ request: Request) async throws -> [Marker] {
    let index = requests.count
    requests.append(request)
    // Deliberately ignores cancellation: store must also reject obsolete responses.
    return try await withCheckedThrowingContinuation { pending[index] = $0 }
  }
  func finish(_ index: Int, _ result: Result<[Marker], any Error>) {
    pending.removeValue(forKey: index)?.resume(with: result)
  }
  func count() -> Int { requests.count }
}

@MainActor
struct PlaceStoreTests {
  @Test func networkRecoveryRetriesFailedReadsWithoutMovingTheMap() async throws {
    let api = DeferredMarkers()
    let store = PlaceStore(api: api)
    store.search("fixture", debounce: false)
    try await waitFor { await api.count() == 1 }
    await api.finish(0, .failure(URLError(.notConnectedToInternet)))
    try await waitFor { store.resultsState == .failed(.requestFailed) }
    let focus = store.focus
    store.retryFailedRequests()
    try await waitFor { await api.count() == 2 }
    store.retryFailedRequests()
    await api.finish(1, .success([sampleMarker(1)]))
    try await waitFor { store.resultsState == .loaded }
    #expect(await api.count() == 2)
    #expect(store.focus == focus && store.browse == .search("fixture"))
    #expect(store.results.map(\.id) == [1])
    store.stop()
  }

  private func waitFor(_ condition: () async -> Bool) async throws {
    for _ in 0..<200 {
      if await condition() { return }
      try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("Timed out waiting for the expected request/state")
  }

  @Test func supersededSearchCannotOverwriteNewerResults() async throws {
    let api = DeferredMarkers()
    let store = PlaceStore(api: api)
    store.search("old", debounce: false)
    try await waitFor { await api.count() == 1 }
    store.search("new", debounce: false)
    try await waitFor { await api.count() == 2 }
    await api.finish(1, .success([sampleMarker(2)]))
    try await waitFor { store.resultsState == .loaded }
    await api.finish(0, .success([sampleMarker(1)]))
    await Task.yield()
    #expect(store.results.map(\.id) == [2])
    store.stop()
  }

  @Test func nearbyStaysAnchoredAndLateLocationCannotHijackSearch() async throws {
    let api = DeferredMarkers()
    let store = PlaceStore(api: api)
    let world = MKMapRect.world
    let original = MapViewport(
      rect: world, center: CLLocationCoordinate2D(latitude: 31, longitude: 121))!
    store.viewportChanged(original, debounce: false)
    try await waitFor { await api.count() == 1 }
    await api.finish(0, .success([]))
    let token = store.nearby(.nursing)
    try await waitFor { await api.count() == 2 }
    store.viewportChanged(
      MapViewport(rect: world, center: CLLocationCoordinate2D(latitude: 40, longitude: -74))!,
      debounce: false)
    try await waitFor { await api.count() == 3 }
    #expect(store.browse == .nearby(category: .nursing, center: original.center, located: false))
    await api.finish(1, .success([sampleMarker(3, category: .nursing)]))
    await api.finish(2, .success([]))
    store.search("new", debounce: false)
    try await waitFor { await api.count() == 4 }
    store.resolveNearbyLocation(GeoPoint(latitude: 50, longitude: 50)!, token: token)
    #expect(store.browse == .search("new"))
    #expect(store.focus == nil)
    await api.finish(3, .success([]))
    store.stop()
  }

  @Test func lateExplicitLocationCannotOverrideSelection() async throws {
    let api = DeferredMarkers()
    let store = PlaceStore(api: api)
    let token = store.beginLocationRequest()
    store.select(store.presentation(sampleMarker(1)))
    let selectedFocus = store.focus
    store.locate(GeoPoint(latitude: 40, longitude: -74)!, token: token)
    #expect(store.focus == selectedFocus)
    #expect(!store.acceptsLocation(token))
    try await waitFor { await api.count() == 1 }
    await api.finish(0, .success([sampleMarker(1)]))
    store.stop()
  }

  @Test func earlyNearbyAcceptsLocationBeforeFirstViewport() async throws {
    let api = DeferredMarkers()
    let store = PlaceStore(api: api)
    let token = store.nearby(.medical)
    #expect(store.pendingNearby == .medical)
    let point = GeoPoint(latitude: 31, longitude: 121)!
    store.resolveNearbyLocation(point, token: token)
    try await waitFor { await api.count() == 1 }
    #expect(store.browse == .nearby(category: .medical, center: point, located: true))
    #expect(store.pendingNearby == nil)
    await api.finish(0, .success([]))
    store.stop()
  }

  @Test func unavailableDetailRestartsInterruptedResults() async throws {
    let api = DeferredMarkers()
    let store = PlaceStore(api: api)
    store.search("places", debounce: false)
    try await waitFor { await api.count() == 1 }
    store.select(store.presentation(sampleMarker(1)))
    try await waitFor { await api.count() == 2 }
    await api.finish(1, .failure(PlaceFailure.unavailable))
    try await waitFor { await api.count() == 3 }
    // Late pre-404 response must not resurrect its stale place.
    await api.finish(0, .success([sampleMarker(1)]))
    await api.finish(2, .success([sampleMarker(2)]))
    try await waitFor { store.resultsState == .loaded }
    store.closeDetail()
    #expect(store.results.map(\.id) == [2])
    store.stop()
  }

  @Test func closingDetailRejectsLateResponseAnd404RemovesMapPin() async throws {
    let api = DeferredMarkers()
    let store = PlaceStore(api: api)
    let marker = sampleMarker(1)
    store.select(store.presentation(marker))
    try await waitFor { await api.count() == 1 }
    store.closeDetail()
    await api.finish(0, .success([marker]))
    await Task.yield()
    #expect(store.selectedPlace == nil)
    store.select(store.presentation(marker))
    try await waitFor { await api.count() == 2 }
    await api.finish(1, .failure(PlaceFailure.unavailable))
    try await waitFor { store.detailState == .failed(.unavailable) }
    #expect(store.mapPlaces.isEmpty)
    store.retryDetail()
    try await waitFor { await api.count() == 3 }
    await api.finish(2, .success([marker]))
    try await waitFor { store.detailState == .loaded }
    #expect(store.mapPlaces.map(\.id) == ["1"])
    store.stop()
  }
}
