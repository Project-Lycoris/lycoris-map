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
  var language = "zh"
  /// The selected venue for an accessible toilet. `nil` means "not specified".
  /// Optional so pre-upgrade drafts without the key still decode.
  var venueType: PlaceVenue? = nil
  /// A raw server venue value this app does not recognize. Kept so editing other
  /// fields never silently rewrites an unknown tag to `other`.
  var unknownVenueType: String? = nil
  var categories: [PlaceCategory]? = nil
  var openingHoursNote: String? = nil
  var selectedCategories: [PlaceCategory] {
    categories.flatMap { $0.isEmpty ? nil : $0 } ?? [category]
  }
  mutating func selectCategories(_ values: [PlaceCategory]) {
    guard let first = values.first else { return }
    category = first
    categories = values
    if !values.contains(.toilet) {
      venueType = nil
      unknownVenueType = nil
    }
  }

  var valid: Bool {
    (openingHoursNote?.unicodeScalars.count ?? 0) <= 1000
      && !title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      && title.unicodeScalars.count <= 120
      && ((openTimeStart.isEmpty && openTimeEnd.isEmpty)
        || (Self.validTime(openTimeStart) && Self.validTime(openTimeEnd)
          && openingHoursMatchClosingDay))
  }

  var closesNextDay: Bool {
    closingDayOverride
      ?? (Self.validTime(openTimeStart) && Self.validTime(openTimeEnd)
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
  init(language _: String) {
    self.language = "zh"
    self.venueType = .other
  }
  init(marker: Marker) {
    title = marker.title
    category = marker.category
    categories = marker.facilityCategories
    openingHoursNote = marker.openingHoursNote ?? ""
    description = marker.description ?? ""
    openTimeStart = marker.openTimeStart ?? ""
    openTimeEnd = marker.openTimeEnd ?? ""
    language = "zh"
    venueType = marker.venue
    // Only an accessible toilet may carry a venue; ignore a stale server value
    // on any other category so it can never be resent.
    unknownVenueType =
      marker.facilityCategories.contains(.toilet) && marker.venue == nil ? marker.venueType : nil
  }

  // Explicit Codable so a pre-upgrade draft JSON (without the new keys) decodes
  // with defaults instead of failing the whole journal.
  private enum CodingKeys: String, CodingKey {
    case title, category, description, openTimeStart, openTimeEnd, language
    case venueType, unknownVenueType, categories, openingHoursNote
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
    categories = try container.decodeIfPresent([PlaceCategory].self, forKey: .categories)
    openingHoursNote = try container.decodeIfPresent(String.self, forKey: .openingHoursNote)
  }
}

struct QueuedContributionPhoto: Codable, Equatable, Identifiable {
  let id: UUID
  let hash: String
  let size: Int
  var valid: Bool {
    (1...(5 * 1024 * 1024)).contains(size)
      && hash.range(of: #"^[a-f0-9]{64}$"#, options: .regularExpression) != nil
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
  var queuedPhotos: [QueuedContributionPhoto]? = nil
  var uploadedPhotoCount: Int? = nil
  var remainingPhotos: [QueuedContributionPhoto] {
    let current = photoID.flatMap { id in
      photoHash.flatMap { hash in
        photoSize.map { QueuedContributionPhoto(id: id, hash: hash, size: $0) }
      }
    }
    return current.map { [$0] + (queuedPhotos ?? []) } ?? (queuedPhotos ?? [])
  }

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
    guard (uploadedPhotoCount ?? 0) >= 0, (queuedPhotos ?? []).allSatisfy(\.valid),
      Set(remainingPhotos.map(\.id)).count == remainingPhotos.count
    else { return false }
    if let upload { return (try? upload.validate(for: self)) != nil }
    return true
  }

  func encodedRequest() throws -> Data {
    var json: [String: Any] = [
      "title": fields.title, "description": fields.description,
      "category": fields.selectedCategories[0].rawValue, "language": "zh",
      "openTimeStart": fields.openTimeStart, "openTimeEnd": fields.openTimeEnd,
    ]
    if let categories = fields.categories { json["categories"] = categories.map(\.rawValue) }
    if let note = fields.openingHoursNote { json["openingHoursNote"] = note }
    // A supplementary toilet can carry a venue even when another type is primary.
    if fields.selectedCategories.contains(.toilet), let venue = fields.venueType {
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
  struct Library {
    var drafts: [ContributionDraft] = []
    var hasUnreadableDrafts = false
  }
  let directory: URL
  init(directory: URL? = nil) {
    self.directory =
      directory
      ?? URL.applicationSupportDirectory.appendingPathComponent(
        "Contribution", isDirectory: true)
  }
  private func draftURL(_ id: UUID) -> URL {
    directory.appendingPathComponent("draft-\(id.uuidString).json")
  }
  private var record: URL { directory.appendingPathComponent("draft.json") }
  func photoURL(_ id: UUID) -> URL { directory.appendingPathComponent("\(id.uuidString).jpg") }

  func load() throws -> ContributionDraft? {
    let path: URL
    if FileManager.default.fileExists(atPath: record.path) {
      path = record
    } else {
      guard
        let latest = try? FileManager.default.contentsOfDirectory(
          at: directory, includingPropertiesForKeys: [.contentModificationDateKey]
        )
        .filter({ $0.lastPathComponent.hasPrefix("draft-") && $0.pathExtension == "json" }).sorted(
          by: { $0.lastPathComponent < $1.lastPathComponent }).last
      else { return nil }
      path = latest
    }
    let value = try JSONDecoder().decode(ContributionDraft.self, from: Data(contentsOf: path))
    guard value.validCheckpoint else { throw ContributionFailure.storage }
    return value
  }
  func list(owner: String, origin: String) throws -> [ContributionDraft] {
    try readLibrary(owner: owner, origin: origin).drafts
  }

  func readLibrary(owner: String, origin: String) throws -> Library {
    try prepare()
    var library = Library()
    var legacyFallback: ContributionDraft?
    if FileManager.default.fileExists(atPath: record.path) {
      do {
        let legacy = try JSONDecoder().decode(
          ContributionDraft.self, from: Data(contentsOf: record))
        guard legacy.validCheckpoint else { throw ContributionFailure.storage }
        if legacy.owner == owner, legacy.origin == origin, legacy.phase != .complete {
          legacyFallback = legacy
        }
        // A newer per-draft checkpoint takes precedence over a leftover legacy
        // file after an interrupted migration.
        if FileManager.default.fileExists(atPath: draftURL(legacy.id).path) {
          let migrated = try JSONDecoder().decode(
            ContributionDraft.self, from: Data(contentsOf: draftURL(legacy.id)))
          guard migrated.validCheckpoint, migrated.id == legacy.id,
            migrated.owner == legacy.owner, migrated.origin == legacy.origin
          else { throw ContributionFailure.storage }
        } else {
          try save(legacy)
        }
        try FileManager.default.removeItem(at: record)
        legacyFallback = nil
      } catch {
        library.hasUnreadableDrafts = true
      }
    }
    let files = try FileManager.default.contentsOfDirectory(
      at: directory, includingPropertiesForKeys: [.contentModificationDateKey]
    )
    .filter { $0.lastPathComponent.hasPrefix("draft-") && $0.pathExtension == "json" }
    .sorted { left, right in
      let a =
        (try? left.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate)
        ?? .distantPast
      let b =
        (try? right.resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate)
        ?? .distantPast
      return a > b
    }
    for url in files {
      do {
        let value = try JSONDecoder().decode(ContributionDraft.self, from: Data(contentsOf: url))
        guard value.owner == owner, value.origin == origin else { continue }
        guard value.validCheckpoint, url == draftURL(value.id) else {
          throw ContributionFailure.storage
        }
        if value.phase != .complete { library.drafts.append(value) }
      } catch {
        // Leave the original checkpoint and photo bytes available for recovery.
        // One unreadable entry must not prevent loading unrelated healthy work.
        library.hasUnreadableDrafts = true
      }
    }
    if let legacyFallback, !library.drafts.contains(where: { $0.id == legacyFallback.id }) {
      library.drafts.append(legacyFallback)
    }
    return library
  }
  func modifiedAt(_ id: UUID) -> Date? {
    try? draftURL(id).resourceValues(forKeys: [.contentModificationDateKey]).contentModificationDate
  }
  func save(_ draft: ContributionDraft) throws {
    try prepare()
    let target = draftURL(draft.id)
    if FileManager.default.fileExists(atPath: target.path) {
      let previous = try Data(contentsOf: target)
      let saved = try? JSONDecoder().decode(ContributionDraft.self, from: previous)
      if saved?.validCheckpoint != true || saved?.id != draft.id
        || saved?.owner != draft.owner || saved?.origin != draft.origin
      {
        // A healthy legacy draft may recover over a damaged UUID checkpoint.
        // Preserve those original bytes before any automatic or user save.
        let backup = directory.appendingPathComponent(
          "unreadable-\(draft.id.uuidString)-\(Self.hash(previous)).json")
        if !FileManager.default.fileExists(atPath: backup.path) {
          try previous.write(
            to: backup, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        }
      }
    }
    try JSONEncoder().encode(draft).write(
      to: target,
      options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
  }
  func remove(_ draft: ContributionDraft) throws {
    if let data = try? Data(contentsOf: record),
      let legacy = try? JSONDecoder().decode(ContributionDraft.self, from: data),
      legacy.id == draft.id, legacy.owner == draft.owner, legacy.origin == draft.origin
    {
      // Discarding a recovered legacy draft must not migrate it back on refresh.
      try FileManager.default.removeItem(at: record)
    }
    let url = draftURL(draft.id)
    if FileManager.default.fileExists(atPath: url.path) {
      try FileManager.default.removeItem(at: url)
    }
    for photo in draft.remainingPhotos { removePhoto(photo.id) }
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
