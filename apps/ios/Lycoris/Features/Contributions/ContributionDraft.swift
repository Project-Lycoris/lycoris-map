import CryptoKit
import Foundation

enum ContributionFailure: Error, Equatable {
  case accountBusy, invalidReceipt, missingPhoto, storage, invalidFields
}

struct ContributionFields: Codable, Equatable {
  var title = ""
  var category = PlaceCategory.toilet
  var description = ""
  var openTimeStart = ""
  var openTimeEnd = ""
  /// UI-only intent. Older drafts and existing places infer the closing day
  /// from the two times; the API continues to receive only HH:mm values.
  var closingDayOverride: Bool? = nil
  var language = "en"
  /// The selected venue for an accessible toilet. `nil` means "not specified".
  /// Optional so pre-upgrade drafts without the key still decode.
  var venueType: PlaceVenue? = nil
  /// A raw server venue value this app does not recognize. Kept so editing other
  /// fields never silently rewrites an unknown tag to `other`.
  var unknownVenueType: String? = nil

  var valid: Bool {
    !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      && title.unicodeScalars.count <= 120
      && ((openTimeStart.isEmpty && openTimeEnd.isEmpty)
        || (Self.validTime(openTimeStart) && Self.validTime(openTimeEnd)
          && openingHoursMatchClosingDay))
  }

  var closesNextDay: Bool {
    closingDayOverride ?? (Self.validTime(openTimeStart) && Self.validTime(openTimeEnd)
      && openTimeEnd < openTimeStart)
  }

  var openingHoursMatchClosingDay: Bool {
    guard Self.validTime(openTimeStart), Self.validTime(openTimeEnd) else { return false }
    // Equal times retain the shared 24-hour contract, with either UI choice.
    return openTimeStart == openTimeEnd || closesNextDay == (openTimeEnd < openTimeStart)
  }

  static func validTime(_ value: String) -> Bool {
    OpeningStatusEngine.isValidTime(value)
  }

  /// A brand-new accessible toilet starts at the `other` default. Old drafts
  /// that decode without the key stay `nil` until the user chooses.
  init(language: String) {
    self.language = language
    self.venueType = .other
  }
  init(marker: Marker) {
    title = marker.title
    category = marker.category
    description = marker.description ?? ""
    openTimeStart = marker.openTimeStart ?? ""
    openTimeEnd = marker.openTimeEnd ?? ""
    language = marker.contentLanguage
    venueType = marker.venue
    // Only an accessible toilet may carry a venue; ignore a stale server value
    // on any other category so it can never be resent.
    unknownVenueType =
      marker.category == .toilet && marker.venue == nil ? marker.venueType : nil
  }

  // Explicit Codable so a pre-upgrade draft JSON (without the new keys) decodes
  // with defaults instead of failing the whole journal.
  private enum CodingKeys: String, CodingKey {
    case title, category, description, openTimeStart, openTimeEnd, language
    case venueType, unknownVenueType
    case closingDayOverride
  }

  init(from decoder: any Decoder) throws {
    let container = try decoder.container(keyedBy: CodingKeys.self)
    title = try container.decodeIfPresent(String.self, forKey: .title) ?? ""
    category =
      try container.decodeIfPresent(PlaceCategory.self, forKey: .category) ?? .toilet
    description = try container.decodeIfPresent(String.self, forKey: .description) ?? ""
    openTimeStart = try container.decodeIfPresent(String.self, forKey: .openTimeStart) ?? ""
    openTimeEnd = try container.decodeIfPresent(String.self, forKey: .openTimeEnd) ?? ""
    closingDayOverride = try container.decodeIfPresent(Bool.self, forKey: .closingDayOverride)
    language = try container.decodeIfPresent(String.self, forKey: .language) ?? "en"
    venueType = try? container.decodeIfPresent(PlaceVenue.self, forKey: .venueType)
    unknownVenueType = try container.decodeIfPresent(String.self, forKey: .unknownVenueType)
  }
}

struct ContributionDraft: Codable, Equatable, Identifiable {
  enum Phase: String, Codable { case draft, creating, editing, uncertainEdit, uploading, complete }
  let id: UUID
  let owner: String
  let origin: String
  var point: GeoPoint
  let original: Marker?
  var fields: ContributionFields
  var phase = Phase.draft
  var requestBody: Data?
  var markerID: Int64?
  var photoID: UUID?
  var photoHash: String?
  var photoSize: Int?
  var upload: UploadReceipt?
  var photoRejected = false

  init(owner: String, origin: String, point: GeoPoint, language: String, marker: Marker? = nil) {
    id = UUID()
    self.owner = owner
    self.origin = origin
    self.point = point
    original = marker
    fields = marker.map(ContributionFields.init) ?? ContributionFields(language: language)
    markerID = marker?.id
  }

  var editable: Bool { phase == .draft }
  var hasChanges: Bool {
    guard let original else { return true }
    var submittedFields = fields
    submittedFields.closingDayOverride = nil
    return submittedFields != ContributionFields(marker: original)
  }
  var canSubmit: Bool { fields.valid && (hasChanges || photoID != nil) }

  var validCheckpoint: Bool {
    guard !owner.isEmpty, !origin.isEmpty,
      GeoPoint(latitude: point.latitude, longitude: point.longitude) != nil,
      markerID == nil || markerID! > 0
    else { return false }
    if [.uploading, .complete].contains(phase), markerID == nil { return false }
    if [.creating, .editing, .uncertainEdit].contains(phase), requestBody == nil { return false }
    if [.editing, .uncertainEdit].contains(phase), original == nil { return false }
    if phase == .creating && original != nil { return false }
    if let original, original.point != point || original.id != markerID { return false }
    if let photoSize, let photoHash, photoID != nil {
      guard (1...(5 * 1024 * 1024)).contains(photoSize),
        photoHash.range(of: #"^[a-f0-9]{64}$"#, options: .regularExpression) != nil
      else { return false }
    } else if photoID != nil || photoSize != nil || photoHash != nil || phase == .uploading {
      return false
    }
    if let upload { return (try? upload.validate(for: self)) != nil }
    return true
  }

  func encodedRequest() throws -> Data {
    var json: [String: Any] = [
      "title": fields.title, "description": fields.description,
      "category": fields.category.rawValue, "language": fields.language,
      "openTimeStart": fields.openTimeStart, "openTimeEnd": fields.openTimeEnd,
    ]
    // Only accessible toilets may carry a venue, and the tag must never be sent
    // for another category (the server clears it then). A new toilet that keeps
    // the default `other` omits the field, which the server defaults to `other`.
    // Only accessible toilets may carry a venue; never send a stale toilet tag
    // for another category (the server clears it then).
    if fields.category == .toilet, let venue = fields.venueType {
      // A new toilet keeping the default `other` omits the field, which the
      // server defaults to `other`; an edit always sends the current value so a
      // deliberate change (including back to `other`) is not lost.
      // An unrecognized server value (venueType == nil) is omitted so PATCH
      // preserves the server's original instead of resending an unknown raw to
      // the server's venue validator.
      if original != nil || venue != .other {
        json["venueType"] = venue.rawValue
      }
    }
    if original == nil {
      json["lat"] = point.latitude
      json["lng"] = point.longitude
      json["clientRequestId"] = id.uuidString
    }
    return try JSONSerialization.data(withJSONObject: json, options: .sortedKeys)
  }
}

struct UploadReceipt: Codable, Equatable {
  let uploadId: String
  let markerId: Int64
  let totalBytes: Int
  let receivedBytes: Int
  let chunkSize: Int
  let status: String
  var complete: Bool { status == "COMPLETED" }

  func validate(for draft: ContributionDraft) throws {
    guard UUID(uuidString: uploadId) != nil, markerId == draft.markerID,
      totalBytes == draft.photoSize, chunkSize == 262_144,
      receivedBytes >= 0, receivedBytes <= totalBytes,
      receivedBytes == totalBytes || receivedBytes % chunkSize == 0,
      ["UPLOADING", "COMPLETED"].contains(status),
      !complete || receivedBytes == totalBytes,
      draft.upload == nil || draft.upload?.uploadId == uploadId,
      receivedBytes >= (draft.upload?.receivedBytes ?? 0)
    else { throw ContributionFailure.invalidReceipt }
  }
}

/// One private, atomic journal. The final encoded image is written first, never
/// regenerated on retry. Neither file participates in device/iCloud backups.
struct ContributionJournal {
  let directory: URL
  init(directory: URL? = nil) {
    self.directory =
      directory
      ?? URL.applicationSupportDirectory.appendingPathComponent(
        "Contribution", isDirectory: true)
  }
  private var record: URL { directory.appendingPathComponent("draft.json") }
  func photoURL(_ id: UUID) -> URL { directory.appendingPathComponent("\(id.uuidString).jpg") }

  func load() throws -> ContributionDraft? {
    guard FileManager.default.fileExists(atPath: record.path) else { return nil }
    let value = try JSONDecoder().decode(ContributionDraft.self, from: Data(contentsOf: record))
    guard value.validCheckpoint else { throw ContributionFailure.storage }
    return value
  }
  func save(_ draft: ContributionDraft) throws {
    try prepare()
    try JSONEncoder().encode(draft).write(
      to: record, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
  }
  func savePhoto(_ data: Data, id: UUID) throws -> (hash: String, size: Int) {
    guard !data.isEmpty, data.count <= 5 * 1024 * 1024 else { throw AccountFailure(status: 413) }
    try prepare()
    try data.write(
      to: photoURL(id), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    // Hash the durable bytes, exactly as subsequent requests will read them.
    let saved = try Data(contentsOf: photoURL(id))
    return (Self.hash(saved), saved.count)
  }
  func photo(_ draft: ContributionDraft) throws -> Data {
    guard let id = draft.photoID, let data = try? Data(contentsOf: photoURL(id)),
      data.count == draft.photoSize, Self.hash(data) == draft.photoHash
    else { throw ContributionFailure.missingPhoto }
    return data
  }
  func removePhoto(_ id: UUID?) {
    if let id { try? FileManager.default.removeItem(at: photoURL(id)) }
  }
  func clear() throws {
    if FileManager.default.fileExists(atPath: directory.path) {
      try FileManager.default.removeItem(at: directory)
    }
  }
  private func prepare() throws {
    try FileManager.default.createDirectory(
      at: directory, withIntermediateDirectories: true,
      attributes: [
        .posixPermissions: 0o700,
        .protectionKey: FileProtectionType.completeUntilFirstUserAuthentication,
      ])
    var location = directory
    var values = URLResourceValues()
    values.isExcludedFromBackup = true
    try location.setResourceValues(values)
  }
  static func hash(_ data: Data) -> String {
    SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
  }
}
