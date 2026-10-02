import Foundation

/// The controlled venue types served for accessible toilets. Unknown or
/// missing server strings are deliberately not mapped here, so an unknown raw
/// value never displays an invented "Other" tag.
enum PlaceVenue: String, Codable, CaseIterable, Sendable, Identifiable {
  case metro
  case hospital
  case mall
  case railwayStation = "railway_station"
  case school
  case park
  case airport
  case publicToilet = "public_toilet"
  case other

  var id: String { rawValue }

  /// SF Symbol for the native venue tag. Uses only system assets.
  var symbol: String {
    switch self {
    case .metro: "tram.fill"
    case .hospital: "cross.case.fill"
    case .mall: "bag.fill"
    case .railwayStation: "train.side.front.car"
    case .school: "graduationcap.fill"
    case .park: "tree.fill"
    case .airport: "airplane"
    case .publicToilet: "toilet.fill"
    case .other: "mappin.and.ellipse"
    }
  }

  func title(language: AppLanguage) -> String {
    String(appLocalized: titleKey, language: language, table: "PlaceMetadata")
  }

  private var titleKey: String.LocalizationValue {
    switch self {
    case .metro: "Metro"
    case .hospital: "Hospital"
    case .mall: "Mall"
    case .railwayStation: "Railway station"
    case .school: "School"
    case .park: "Park"
    case .airport: "Airport"
    case .publicToilet: "Public toilet"
    case .other: "Other"
    }
  }
}
