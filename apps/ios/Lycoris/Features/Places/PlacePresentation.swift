import Foundation

struct PlacePresentation: Identifiable, Equatable, Sendable {
  let id: String
  let title: String
  let detailTitle: String
  let distance: String
  let openingHours: String
  let description: String
  let photoAsset: String?
  let latitude: Double
  let longitude: Double
  var category: PlaceCategory = .toilet
  var imageURL: URL? = nil
  var distanceReference: String? = nil
  /// Recognized venue tag for the tag chip; `nil` for missing or unknown values.
  var venue: PlaceVenue? = nil
  /// The raw server venue string, retained so unknown values survive edits
  /// instead of being silently rewritten to `other`.
  var venueRaw: String? = nil
  /// The raw server time zone. Never written back; used only for status.
  var hoursTimezone: String? = nil
  /// The original server `HH:mm` values, echoed into the editor when valid.
  var rawStart: String? = nil
  var rawEnd: String? = nil

  var point: GeoPoint? { GeoPoint(latitude: latitude, longitude: longitude) }
  var hasPhoto: Bool { photoAsset != nil || imageURL != nil }

  /// The real-time status for the given instant, computed from the server zone.
  func openingStatus(at now: Date) -> OpeningStatus {
    OpeningStatusEngine.status(
      start: rawStart, end: rawEnd, timeZone: hoursTimezone, now: now)
  }

  init(marker: Marker, origin: GeoPoint?, located: Bool, baseURL: URL?) {
    id = String(marker.id)
    title = marker.title
    detailTitle = marker.title
    category = marker.category
    latitude = marker.lat
    longitude = marker.lng
    description = marker.description ?? ""
    photoAsset = nil
    imageURL = Self.imageURL(marker.markImage, baseURL: baseURL)
    rawStart = marker.openTimeStart
    rawEnd = marker.openTimeEnd
    openingHours = Self.hours(start: marker.openTimeStart, end: marker.openTimeEnd)
    venueRaw = marker.venueType
    venue = marker.venue
    hoursTimezone = marker.hoursTimezone
    if let origin, let point = marker.point {
      let meters = origin.distance(to: point)
      distance =
        meters < 1000 ? "\(Int(meters.rounded()))m" : String(format: "%.1fkm", meters / 1000)
      distanceReference =
        located
        ? nil
        : String(appLocalized: "Straight-line distance from map center")
    } else {
      distance = ""
      distanceReference = nil
    }
  }

  // Explicit design-only values, also used by SwiftUI previews.
  init(
    id: String, title: String, detailTitle: String, distance: String, openingHours: String,
    description: String, photoAsset: String, latitude: Double, longitude: Double
  ) {
    self.id = id
    self.title = title
    self.detailTitle = detailTitle
    self.distance = distance
    self.openingHours = openingHours
    self.description = description
    self.photoAsset = photoAsset
    self.latitude = latitude
    self.longitude = longitude
  }

  static func hours(start: String?, end: String?) -> String {
    guard let start, let end,
      OpeningStatusEngine.isValidTime(start), OpeningStatusEngine.isValidTime(end)
    else {
      return String(appLocalized: "Hours not provided")
    }
    if start == end { return String(appLocalized: "Open 24 hours") }
    if end < start {
      return String(format: String(appLocalized: "%@–%@ (next day)", table: "OpeningHours"), start, end)
    }
    return "\(start)–\(end)"
  }

  static func imageURL(_ path: String?, baseURL: URL?) -> URL? {
    guard let path, let baseURL,
      path.range(of: #"^/uploads/markers/[A-Za-z0-9_.-]+$"#, options: .regularExpression) != nil,
      ![".", ".."].contains(String(path.split(separator: "/").last ?? ""))
    else { return nil }
    return URL(string: path, relativeTo: baseURL)?.absoluteURL
  }

  /// Share only the identifier. The receiving app rechecks access to the place.
  var shareURL: URL? { Int64(id).flatMap(PlaceLink.url(id:)) }
}
