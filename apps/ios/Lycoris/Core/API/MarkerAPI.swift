import Foundation

/// Public read DTO. Rust identifiers stay Int64 all the way to the URL.
struct Marker: Codable, Equatable, Sendable {
  let id: Int64
  let version: Int64
  let lat: Double
  let lng: Double
  let category: PlaceCategory
  let title: String
  let description: String?
  let openTimeStart: String?
  let openTimeEnd: String?
  let markImage: String?
  let contentLanguage: String
  var isPublic: Bool? = nil
  var reviewStatus: String? = nil
  /// Controlled venue tag. Kept as a raw `String?` so a future/unknown server
  /// value can never fail the whole list decode; use `venue` for display.
  var venueType: String? = nil
  /// Server-owned IANA opening-hours time zone. Read-only: never inferred from
  /// the device and never written back by the app.
  var hoursTimezone: String? = nil
  var categories: [PlaceCategory]? = nil
  var openingHoursNote: String? = nil
  var photos: [MarkerPhoto]? = nil
  var facilityCategories: [PlaceCategory] {
    categories.flatMap { $0.isEmpty ? nil : $0 } ?? [category]
  }

  var point: GeoPoint? { GeoPoint(latitude: lat, longitude: lng) }
  /// Only an accessible toilet may show or speak a venue tag. A stale or
  /// malformed venue on another category is ignored rather than displayed.
  var venue: PlaceVenue? {
    guard facilityCategories.contains(.toilet) else { return nil }
    return venueType.flatMap(PlaceVenue.init(rawValue:))
  }
}

struct MarkerPhoto: Codable, Equatable, Sendable {
  let id: Int64
  let url: String
  let sortOrder: Int64
}

enum PlaceCategory: String, Codable, CaseIterable, Sendable {
  case toilet = "accessible_toilet"
  case nursing = "baby_room"
  case medical = "friendly_clinic"
  case other = "self_definition"

  var title: String {
    switch self {
    case .toilet: String(appLocalized: "Accessible Toilets")
    case .nursing: String(appLocalized: "Nursing Rooms")
    case .medical: String(appLocalized: "Medical Institutions")
    case .other: String(appLocalized: "Places")
    }
  }
  var image: String {
    switch self {
    case .toilet: "Toilet"
    case .nursing: "Nursing"
    case .medical: "Medical"
    case .other: "PlacePin"
    }
  }
  var tint: String {
    switch self {
    case .toilet, .other: "ToiletTint"
    case .nursing: "NursingTint"
    case .medical: "MedicalTint"
    }
  }
  var pinAsset: String {
    switch self {
    case .toilet: "PlacePin"
    case .nursing: "PlacePinNursing"
    case .medical: "PlacePinMedical"
    case .other: "PlacePinOther"
    }
  }
}

struct MarkerBounds: Equatable, Sendable {
  let south: Double
  let west: Double
  let north: Double
  let east: Double
}

enum MarkerQuery: Equatable, Sendable {
  case viewport(MarkerBounds)
  case search(String)
  case nearby(GeoPoint, PlaceCategory, radius: Int = 1000)
}

enum PlaceFailure: Error, Equatable {
  case unconfigured, unavailable, invalidResponse, requestFailed, network, timeout, unauthenticated,
    forbidden, rateLimited, server

  static func http(_ status: Int) -> PlaceFailure {
    switch status {
    case 0: .network
    case -1: .unconfigured
    case 401: .unauthenticated
    case 403: .forbidden
    case 404: .unavailable
    case 408, 504: .timeout
    case 429: .rateLimited
    case 500...599: .server
    default: .requestFailed
    }
  }

  var message: String {
    switch self {
    case .unconfigured: String(appLocalized: "The map service is not configured yet.")
    case .unavailable: String(appLocalized: "This place is no longer available.")
    case .network: String(appLocalized: "Network unavailable. Check your connection and try again.")
    case .timeout: String(appLocalized: "Loading places timed out. Try again.")
    case .unauthenticated: String(appLocalized: "Your session expired. Please log in again.")
    case .forbidden: String(appLocalized: "You do not have permission to view these places.")
    case .rateLimited: String(appLocalized: "Too many requests. Please wait before trying again.")
    case .server:
      String(appLocalized: "The place service is temporarily unavailable. Try again later.")
    case .invalidResponse: String(appLocalized: "The place data could not be read. Try again.")
    case .requestFailed:
      String(appLocalized: "Could not load places. Please try again.")
    }
  }
}

/// Keeps server correlation metadata for diagnostics without displaying server bodies.
struct MarkerRequestFailure: Error, Equatable, Sendable {
  let failure: PlaceFailure
  let status: Int
  let requestID: String?
}

protocol MarkerServing: Sendable {
  var baseURL: URL? { get }
  func markers(_ query: MarkerQuery, language: String) async throws -> [Marker]
  func detail(id: Int64, language: String) async throws -> Marker
}

struct MarkerAPI: MarkerServing {
  let baseURL: URL?
  let session: URLSession

  init(
    baseURL: URL? = (try? AppConfiguration.bundled())?.apiBaseURL,
    session: URLSession? = nil
  ) {
    self.baseURL = baseURL
    if let session {
      self.session = session
    } else {
      let configuration = URLSessionConfiguration.ephemeral
      configuration.httpShouldSetCookies = false
      configuration.httpCookieStorage = nil
      configuration.urlCredentialStorage = nil
      configuration.timeoutIntervalForRequest = 15
      configuration.waitsForConnectivity = true
      configuration.timeoutIntervalForResource = 60
      self.session = URLSession(configuration: configuration)
    }
  }

  func markers(_ query: MarkerQuery, language: String) async throws -> [Marker] {
    let request = try request(for: query, language: language)
    let markers: [Marker] = try await read(request)
    guard markers.allSatisfy({ $0.id > 0 && $0.point != nil }) else {
      throw PlaceFailure.invalidResponse
    }
    // No pagination wrapper is used by these endpoints.
    var seen = Set<Int64>()
    return markers.filter { seen.insert($0.id).inserted }
  }

  func detail(id: Int64, language: String) async throws -> Marker {
    guard id > 0 else { throw PlaceFailure.unavailable }
    let marker: Marker = try await read(makeRequest(path: "\(id)", query: ["lang": language]))
    guard marker.id == id, marker.point != nil else { throw PlaceFailure.invalidResponse }
    return marker
  }

  func request(for query: MarkerQuery, language: String) throws -> URLRequest {
    var params = ["lang": language]
    let path: String
    switch query {
    case .viewport(let bounds):
      path = "viewport"
      params.merge([
        "minLat": String(bounds.south), "minLng": String(bounds.west),
        "maxLat": String(bounds.north), "maxLng": String(bounds.east),
      ]) { _, new in new }
    case .search(let term):
      path = "search"
      params["q"] = term.trimmingCharacters(in: .whitespacesAndNewlines)
    case .nearby(let point, let category, let radius):
      guard (1...50_000).contains(radius) else { throw PlaceFailure.invalidResponse }
      path = "nearby"
      params.merge([
        "lat": String(point.latitude), "lng": String(point.longitude),
        "radius": String(radius), "category": category.rawValue,
      ]) { _, new in new }
    }
    return try makeRequest(path: path, query: params)
  }

  private func makeRequest(path: String, query: [String: String]) throws -> URLRequest {
    guard let baseURL else { throw PlaceFailure.unconfigured }
    var components = URLComponents(
      url: baseURL.appendingPathComponent("api/markers/\(path)"), resolvingAgainstBaseURL: false)!
    components.queryItems = query.sorted { $0.key < $1.key }.map {
      URLQueryItem(name: $0.key, value: $0.value)
    }
    guard let url = components.url else { throw PlaceFailure.invalidResponse }
    var request = URLRequest(url: url)
    request.setValue("application/json", forHTTPHeaderField: "Accept")
    return request
  }

  private func read<Value: Decodable>(_ request: URLRequest) async throws -> Value {
    let data: Data
    let response: URLResponse
    do { (data, response) = try await session.data(for: request) } catch let error as URLError {
      if error.code == .cancelled { throw CancellationError() }
      throw error.code == .timedOut ? PlaceFailure.timeout : PlaceFailure.network
    }
    try Task.checkCancellation()
    guard let response = response as? HTTPURLResponse else { throw PlaceFailure.invalidResponse }
    func failure(_ reason: PlaceFailure) -> MarkerRequestFailure {
      MarkerRequestFailure(
        failure: reason, status: response.statusCode,
        requestID: response.value(forHTTPHeaderField: "X-Request-ID"))
    }
    guard (200..<300).contains(response.statusCode) else {
      throw failure(.http(response.statusCode))
    }
    do { return try JSONDecoder().decode(Value.self, from: data) } catch {
      throw failure(.invalidResponse)
    }
  }
}
