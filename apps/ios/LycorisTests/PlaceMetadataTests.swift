import Foundation
import Testing
import UIKit

@testable import Lycoris

/// Contract and behavior coverage for the place venue tag and the server-time
/// opening status. These tests mirror `reference/openingStatus.ts` and the
/// controlled venue contract; they never touch a network or a real backend.
struct PlaceMetadataTests {
  // MARK: - Venue DTO contract

  @Test func decodesEveryControlledVenueAndBothLanguages() throws {
    let expected: [(String, PlaceVenue)] = [
      ("metro", .metro), ("hospital", .hospital), ("mall", .mall),
      ("railway_station", .railwayStation), ("school", .school), ("other", .other),
      ("airport", .airport), ("public_toilet", .publicToilet),
    ]
    for (raw, venue) in expected {
      let marker = try decodeMarker(venueType: raw)
      #expect(marker.venue == venue)
      #expect(marker.venueType == raw)
      #expect(PlaceVenue(rawValue: raw) == venue)
      let en = venue.title(language: .english)
      let zh = venue.title(language: .chinese)
      #expect(!en.isEmpty && !zh.isEmpty)
      #expect(en != zh)
    }
    #expect(PlaceVenue.allCases.count == 9)
  }

  @Test func newVenuesSurviveEditingAndDraftRestoration() throws {
    for (raw, english, chinese) in [
      ("airport", "Airport", "飞机场"), ("public_toilet", "Public toilet", "公共卫生间"),
    ] {
      let marker = try decodeMarker(venueType: raw)
      let venue = try #require(marker.venue)
      #expect(venue.title(language: .english) == english)
      #expect(venue.title(language: .chinese) == chinese)
      #expect(UIImage(systemName: venue.symbol) != nil)
      let place = PlacePresentation(marker: marker, origin: nil, located: false, baseURL: nil)
      #expect(PlaceAccessibility.placeLabel(place, language: .english).contains(english))
      #expect(PlaceAccessibility.placeLabel(place, language: .chinese).contains(chinese))

      var edit = draft(original: marker)
      #expect(edit.fields.venueType == venue && edit.fields.unknownVenueType == nil)
      edit.fields.title = "Updated title"
      let restored = try JSONDecoder().decode(
        ContributionDraft.self, from: JSONEncoder().encode(edit))
      #expect(restored.fields.venueType == venue)
      #expect(try json(restored)["venueType"] as? String == raw)

      var new = draft()
      new.fields.title = "New toilet"
      new.fields.venueType = venue
      #expect(try json(new)["venueType"] as? String == raw)
    }
  }

  @Test func missingNullAndUnknownVenueNeverFailDecodingNorInventATag() throws {
    // Old responses/persisted markers without the key.
    let missing = try decodeMarker(venueType: nil)
    #expect(missing.venueType == nil && missing.venue == nil)
    // Explicit null.
    let null = try decodeMarker(json: #""venueType":null"#)
    #expect(null.venueType == nil && null.venue == nil)
    // A future unknown string must decode as raw text, not fail the list.
    let unknown = try decodeMarker(venueType: "space_station")
    #expect(unknown.venueType == "space_station")
    #expect(unknown.venue == nil)
    // A whole list with an unknown value still decodes.
    let list = try JSONDecoder().decode(
      [Marker].self,
      from: Data(
        #"""
        [{"id":1,"version":1,"lat":31.2,"lng":121.4,"category":"accessible_toilet","title":"A","contentLanguage":"en","venueType":"space_station"},
         {"id":2,"version":1,"lat":31.2,"lng":121.4,"category":"accessible_toilet","title":"B","contentLanguage":"en"}]
        """#.utf8))
    #expect(list.count == 2)
    #expect(list[0].venue == nil && list[1].venue == nil)
  }

  @Test func venueIsIgnoredOnNonToiletCategories() throws {
    // A stale/corrupt venue on a non-toilet must never be displayed or spoken.
    let nursing = try decodeMarker(
      venueType: "metro", category: "baby_room")
    #expect(nursing.venueType == "metro")
    #expect(nursing.venue == nil)
    let place = PlacePresentation(marker: nursing, origin: nil, located: false, baseURL: nil)
    #expect(place.venue == nil)
    let label = PlaceAccessibility.placeLabel(place, language: .english, now: .now)
    #expect(!label.contains("Metro"))
    // And it cannot leak into a contribution payload.
    let edit = draft(original: nursing)
    #expect(edit.fields.venueType == nil && edit.fields.unknownVenueType == nil)
    #expect(try json(edit)["venueType"] == nil)
  }

  @Test func hoursTimezoneDecodesButIsNeverRequired() throws {
    let withZone = try decodeMarker(json: #""hoursTimezone":"Asia/Shanghai""#)
    #expect(withZone.hoursTimezone == "Asia/Shanghai")
    let without = try decodeMarker(venueType: nil)
    #expect(without.hoursTimezone == nil)
  }

  // MARK: - Opening status boundaries (mirrors reference/openingStatus.ts)

  private let shanghai = "Asia/Shanghai"

  @Test func statusUsesTheServerZoneAndWarnsExactlyThirtyMinutesBeforeClosing() {
    func status(_ iso: String) -> OpeningStatus {
      OpeningStatusEngine.status(
        start: "09:00", end: "22:00", timeZone: shanghai,
        now: Date(timeIntervalSince1970: isoDate(iso)))
    }
    #expect(status("2026-09-20T13:29:59Z") == .open)
    #expect(status("2026-09-20T13:30:00Z") == .closingSoon)
    #expect(status("2026-09-20T13:59:59Z") == .closingSoon)
    #expect(status("2026-09-20T14:00:00Z") == .closed)
    #expect(status("2026-09-20T00:59:59Z") == .closed)
    #expect(status("2026-09-20T01:00:00Z") == .open)
  }

  @Test func statusHandlesOvernightMidnightClosingAndAllDayAvailability() {
    let overnight = (
      start: "22:00", end: "06:00", zone: shanghai as String?
    )
    func status(_ iso: String, _ hours: (start: String, end: String, zone: String?))
      -> OpeningStatus
    {
      OpeningStatusEngine.status(
        start: hours.start, end: hours.end, timeZone: hours.zone,
        now: Date(timeIntervalSince1970: isoDate(iso)))
    }
    #expect(status("2026-09-20T15:59:59Z", overnight) == .open)
    #expect(status("2026-09-20T21:30:00Z", overnight) == .closingSoon)
    #expect(status("2026-09-20T22:00:00Z", overnight) == .closed)
    #expect(status("2026-09-20T15:30:00Z", ("09:00", "00:00", shanghai)) == .closingSoon)
    #expect(status("2026-09-20T00:45:00Z", ("09:00", "09:00", shanghai)) == .open)
  }

  @Test func invalidOrAbsentHoursAreUnknownAndMissingZoneIsScheduled() {
    let now = Date(timeIntervalSince1970: isoDate("2026-09-20T13:45:00Z"))
    #expect(
      OpeningStatusEngine.status(start: nil, end: "22:00", timeZone: shanghai, now: now) == .unknown
    )
    #expect(
      OpeningStatusEngine.status(start: "24:00", end: "22:00", timeZone: shanghai, now: now)
        == .unknown)
    #expect(
      OpeningStatusEngine.status(start: "09:00", end: "9:00", timeZone: shanghai, now: now)
        == .unknown)
    #expect(
      OpeningStatusEngine.status(start: "09:00", end: "22:00", timeZone: nil, now: now)
        == .scheduled)
    #expect(
      OpeningStatusEngine.status(start: "09:00", end: "22:00", timeZone: "", now: now) == .scheduled
    )
    #expect(
      OpeningStatusEngine.status(start: "09:00", end: "22:00", timeZone: "not-a-zone", now: now)
        == .scheduled)
    // A scheduled status must not claim open/closed in either language.
    #expect(OpeningStatus.scheduled.label(language: .english).isEmpty)
    #expect(OpeningStatus.scheduled.label(language: .chinese).isEmpty)
    #expect(OpeningStatus.unknown.label(language: .english).isEmpty)
    // An invalid instant cannot place a valid schedule on the local clock.
    #expect(
      OpeningStatusEngine.status(
        start: "09:00", end: "22:00", timeZone: shanghai,
        now: Date(timeIntervalSinceReferenceDate: .nan)) == .scheduled)
    #expect(
      OpeningStatusEngine.status(
        start: "09:00", end: "22:00", timeZone: shanghai,
        now: Date(timeIntervalSinceReferenceDate: .infinity)) == .scheduled)
    // The equal-time (24h) rule still resolves to open without needing a clock.
    #expect(
      OpeningStatusEngine.status(
        start: "09:00", end: "09:00", timeZone: shanghai,
        now: Date(timeIntervalSinceReferenceDate: .nan)) == .open)
  }

  @Test func scheduledKeepsAnEmphasisTintAndUnknownStaysNeutral() {
    // The valid planned time is emphasized (accent), but never claims open/closed.
    #expect(OpeningStatus.scheduled.usesStatusColor)
    #expect(!OpeningStatus.unknown.usesStatusColor)
  }

  @Test func closingSoonIsTextuallyDistinctFromOpen() {
    #expect(OpeningStatus.open.label(language: .english) == "Open now")
    #expect(OpeningStatus.closingSoon.label(language: .english) == "Closing soon")
    #expect(
      OpeningStatus.open.label(language: .english)
        != OpeningStatus.closingSoon.label(language: .english))
    #expect(
      OpeningStatus.closingSoon.label(language: .chinese)
        != OpeningStatus.open.label(language: .chinese))
  }

  @Test func deviceTimeZoneDoesNotAffectARemotePlacesStatus() {
    // The same instant is open in Shanghai but closed in New York for 09:00–22:00.
    let now = Date(timeIntervalSince1970: isoDate("2026-09-20T13:45:00Z"))
    #expect(
      OpeningStatusEngine.status(start: "09:00", end: "22:00", timeZone: "Asia/Shanghai", now: now)
        == .closingSoon)
    #expect(
      OpeningStatusEngine.status(
        start: "09:00", end: "22:00", timeZone: "America/New_York", now: now) == .open)
  }

  @Test func statusFollowsIanaDaylightSavingNotAFixedOffset() {
    // New York is UTC-4 in July (EDT) and UTC-5 in January (EST).
    #expect(
      OpeningStatusEngine.status(
        start: "09:00", end: "22:00", timeZone: "America/New_York",
        now: Date(timeIntervalSince1970: isoDate("2026-07-02T01:45:00Z"))) == .closingSoon)
    #expect(
      OpeningStatusEngine.status(
        start: "09:00", end: "22:00", timeZone: "America/New_York",
        now: Date(timeIntervalSince1970: isoDate("2026-01-02T02:45:00Z"))) == .closingSoon)
  }

  @Test func statusLabelsAreTextDistinctNotColorOnly() {
    #expect(OpeningStatus.open.label(language: .english) == "Open now")
    #expect(OpeningStatus.closingSoon.label(language: .english) == "Closing soon")
    #expect(OpeningStatus.closed.label(language: .english) == "Closed now")
    #expect(
      OpeningStatus.open.label(language: .chinese) != OpeningStatus.open.label(language: .english))
    #expect(
      OpeningStatus.closed.label(language: .chinese)
        != OpeningStatus.closed.label(language: .english))
    #expect(OpeningStatus.open.usesStatusColor)
    #expect(OpeningStatus.closingSoon.usesStatusColor)
    #expect(OpeningStatus.scheduled.usesStatusColor)
    #expect(!OpeningStatus.unknown.usesStatusColor)
  }

  // MARK: - Presentation

  @Test func presentationExposesVenueStatusAndRawValues() throws {
    let marker = try decodeMarker(
      venueType: "metro",
      json: #""openTimeStart":"09:00","openTimeEnd":"22:00","hoursTimezone":"Asia/Shanghai""#)
    let place = PlacePresentation(marker: marker, origin: nil, located: false, baseURL: nil)
    #expect(place.venue == .metro)
    #expect(place.venueRaw == "metro")
    #expect(place.hoursTimezone == "Asia/Shanghai")
    #expect(place.rawStart == "09:00" && place.rawEnd == "22:00")
    #expect(
      place.openingStatus(at: Date(timeIntervalSince1970: isoDate("2026-09-20T13:45:00Z")))
        == .closingSoon)
  }

  // MARK: - Accessibility

  @Test func accessibilityLabelAddsVenueAndRealStatusButKeepsTheRest() throws {
    let marker = try decodeMarker(
      venueType: "school",
      json: #""openTimeStart":"09:00","openTimeEnd":"22:00","hoursTimezone":"Asia/Shanghai""#)
    let place = PlacePresentation(marker: marker, origin: nil, located: false, baseURL: nil)
    let now = Date(timeIntervalSince1970: isoDate("2026-09-20T13:45:00Z"))
    let en = PlaceAccessibility.placeLabel(place, language: .english, now: now)
    #expect(en.contains("School"))
    #expect(en.contains(place.title))
    #expect(en.contains("09:00–22:00"))
    #expect(en.hasSuffix("Closing soon"))
    // Category, name and hours survive; only the decorative icon is skipped.
    #expect(en.hasPrefix(PlaceAccessibility.categoryLabel(.toilet, language: .english)))

    // An unknown/absent venue adds no tag text.
    let noVenue = PlacePresentation(
      marker: try decodeMarker(venueType: nil), origin: nil, located: false, baseURL: nil)
    #expect(!PlaceAccessibility.placeLabel(noVenue, language: .english, now: now).contains("Other"))
  }

  // MARK: - Contribution drafts and payloads

  @Test func oldDraftWithoutNewKeysStillDecodesWithDefaults() throws {
    let legacy =
      #"{"id":"\#(UUID().uuidString)","owner":"a","origin":"https://i.example.invalid","point":{"latitude":1,"longitude":1},"fields":{"title":"Legacy","category":"accessible_toilet","description":"","openTimeStart":"","openTimeEnd":"","language":"en"},"phase":"draft","photoRejected":false}"#
    let draft = try JSONDecoder().decode(ContributionDraft.self, from: Data(legacy.utf8))
    #expect(draft.fields.title == "Legacy")
    #expect(draft.fields.venueType == nil && draft.fields.unknownVenueType == nil)
    #expect(draft.validCheckpoint)
  }

  @Test func newDraftPayloadOmitsDefaultOtherButSendsOtherChosenValues() throws {
    var new = draft()
    new.fields.title = "New toilet"
    // A brand-new toilet truly starts at the other default and omits the field;
    // the server defaults it to other.
    #expect(new.fields.venueType == .other)
    #expect(try json(new)["venueType"] == nil)
    // A chosen non-other value is sent.
    new.fields.venueType = .metro
    #expect(try json(new)["venueType"] as? String == "metro")
  }

  @Test func newVenueTypesSurviveDraftPersistenceAndBothWritePayloads() throws {
    for venue in [PlaceVenue.publicToilet, .airport] {
      let marker = try decodeMarker(venueType: venue.rawValue)
      var edit = draft(original: marker)
      edit.fields.title = "Updated"
      let restored = try JSONDecoder().decode(
        ContributionDraft.self, from: JSONEncoder().encode(edit))
      #expect(restored.fields.venueType == venue)
      #expect(try json(restored)["venueType"] as? String == venue.rawValue)
      var new = draft()
      new.fields.title = "New toilet"
      new.fields.venueType = venue
      #expect(try json(new)["venueType"] as? String == venue.rawValue)
    }
  }

  @Test func legacyDraftWithNoVenueKeyStaysNil() throws {
    let legacy =
      #"{"title":"Legacy","category":"accessible_toilet","description":"","openTimeStart":"","openTimeEnd":"","language":"en"}"#
    let fields = try JSONDecoder().decode(ContributionFields.self, from: Data(legacy.utf8))
    // An old draft must not be silently upgraded to other.
    #expect(fields.venueType == nil)
    #expect(fields.unknownVenueType == nil)
  }

  @Test func editPatchSendsCurrentVenueAndNeverSendsItForOtherCategories() throws {
    let original = try decodeMarker(
      venueType: "metro",
      json: #""openTimeStart":"09:00","openTimeEnd":"22:00","hoursTimezone":"Asia/Shanghai""#)
    var edit = draft(original: original)
    #expect(edit.fields.venueType == .metro)
    // Switching off toilet through the store clears the stale tag and the
    // payload omits it.
    edit.fields.category = .nursing
    edit.fields = ContributionStore.switchingCategory(edit.fields, from: .toilet)
    #expect(edit.fields.venueType == nil && edit.fields.unknownVenueType == nil)
    #expect(try json(edit)["venueType"] == nil)
    // Switching back to toilet defaults to other with no stale tag.
    edit.fields.category = .toilet
    edit.fields = ContributionStore.switchingCategory(edit.fields, from: .nursing)
    #expect(edit.fields.venueType == .other)
    #expect(try json(edit)["venueType"] as? String == "other")
  }

  @Test func unknownServerVenueIsPreservedLocallyButOmittedFromThePayload() throws {
    let unknown = try decodeMarker(
      venueType: "space_station",
      json: #""openTimeStart":"09:00","openTimeEnd":"22:00","hoursTimezone":"Asia/Shanghai""#)
    var edit = draft(original: unknown)
    #expect(edit.fields.venueType == nil)
    #expect(edit.fields.unknownVenueType == "space_station")
    // An unrelated text edit preserves the raw locally for display, but the
    // payload omits it so PATCH keeps the server's original venue field
    // untouched instead of resending an unknown raw.
    edit.fields.title = "Renamed"
    #expect(edit.fields.unknownVenueType == "space_station")
    #expect(try json(edit)["venueType"] == nil)
    // A new place with no known/unknown venue never invents a tag.
    var fresh = draft()
    fresh.fields.title = "Fresh"
    fresh.fields.venueType = nil
    #expect(try json(fresh)["venueType"] == nil)
  }

  @Test func changingOnlyTheVenueStillProducesChangesAndASubmittablePayload() throws {
    let original = try decodeMarker(
      venueType: "other",
      json: #""openTimeStart":"09:00","openTimeEnd":"22:00","hoursTimezone":"Asia/Shanghai""#)
    var edit = draft(original: original)
    #expect(edit.hasChanges == false)
    edit.fields.venueType = .school
    #expect(edit.hasChanges)
    #expect(edit.canSubmit)
    #expect(try json(edit)["venueType"] as? String == "school")
  }

  @Test func unicodeDigitsAreNotValidTimes() {  // JavaScript's digit class is ASCII-only, and so is the shared contract.
    // Swift's regex digit class would accept Arabic-Indic/full-width digits, so
    // the engine must reject them and never treat them as all-day open.
    for value in ["0١:0١", "٠٩:٠٠", "０９:００", "09：00"] {
      #expect(!OpeningStatusEngine.isValidTime(value), Comment(rawValue: value))
      #expect(!ContributionFields.validTime(value), Comment(rawValue: value))
    }
    #expect(OpeningStatusEngine.isValidTime("09:00"))
    #expect(ContributionFields.validTime("23:59"))
  }

  @Test func upgradeADraftNeverChangesAnInFlightRetryPayload() throws {
    // A pre-upgrade journal whose frozen requestBody was produced without the
    // new venue keys. Decoding and re-saving it must preserve those bytes
    // verbatim so an in-flight retry sends exactly the frozen proposal.
    let legacyBody = Data(
      #"{"category":"accessible_toilet","description":"","language":"en","openTimeEnd":"22:00","openTimeStart":"09:00","title":"Legacy"}"#
        .utf8)
    let original =
      #"{"id":55,"version":1,"lat":31.2,"lng":121.4,"category":"accessible_toilet","title":"Legacy","description":"","openTimeStart":"09:00","openTimeEnd":"22:00","contentLanguage":"en"}"#
    let legacy =
      #"{"id":"\#(UUID().uuidString)","owner":"a","origin":"https://i.example.invalid","point":{"latitude":31.2,"longitude":121.4},"original":\#(original),"fields":{"title":"Legacy","category":"accessible_toilet","description":"","openTimeStart":"09:00","openTimeEnd":"22:00","language":"en"},"phase":"editing","requestBody":"\#(legacyBody.base64EncodedString())","markerID":55,"photoRejected":false}"#
    let draft = try JSONDecoder().decode(ContributionDraft.self, from: Data(legacy.utf8))
    #expect(draft.requestBody == legacyBody)
    #expect(draft.fields.venueType == nil)
    #expect(draft.validCheckpoint)
    // Round-trip the journal and confirm the frozen bytes and checkpoint survive.
    let journal = ContributionJournal(
      directory: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
    defer { try? journal.clear() }
    try journal.save(draft)
    let reloaded = try #require(try journal.load())
    #expect(reloaded.requestBody == legacyBody)
    #expect(reloaded.fields.venueType == nil)
    #expect(reloaded.validCheckpoint)
  }

  @Test func hoursTimezoneIsReadOnlyAndNeverWritten() throws {
    let original = try decodeMarker(
      venueType: "metro",
      json: #""openTimeStart":"09:00","openTimeEnd":"22:00","hoursTimezone":"Asia/Shanghai""#)
    var edit = draft(original: original)
    edit.fields.title = "Renamed"
    let body = try json(edit)
    #expect(body["hoursTimezone"] == nil)
    #expect(body["isActive"] == nil)
  }

  // MARK: - Real store update regression

  @Test @MainActor func storeUpdateOnALegacyDraftKeepsVenueAbsentUntilChosen() async throws {
    // Restore a pre-upgrade edit whose original has no venue, then make an
    // ordinary text update through the real store: the payload must still omit
    // venueType so the server's later classification is not overwritten.
    let api = MetadataAccountFixture()
    let journal = ContributionJournal(
      directory: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
    defer { try? journal.clear() }
    let account = AccountStore(api: api)
    let store = ContributionStore(journal: journal)
    store.connect(account, monitorNetwork: false)
    await account.restore()
    try await store.edit(55)
    let draft = try #require(store.draft)
    #expect(draft.fields.venueType == nil && draft.fields.unknownVenueType == nil)

    var fields = try #require(store.draft?.fields)
    fields.title = "Renamed only"
    store.update(fields)
    #expect(store.draft?.fields.venueType == nil)
    let updated = try #require(store.draft)
    let body = try #require(
      JSONSerialization.jsonObject(with: updated.encodedRequest()) as? [String: Any])
    #expect(body["venueType"] == nil)
    #expect(body["title"] as? String == "Renamed only")

    // An explicit choice of Other is sent.
    fields.venueType = .other
    store.update(fields)
    let chosenDraft = try #require(store.draft)
    let chosen = try #require(
      JSONSerialization.jsonObject(with: chosenDraft.encodedRequest()) as? [String: Any])
    #expect(chosen["venueType"] as? String == "other")
    store.setActive(false)
  }

  // MARK: - Helpers
  private func decodeMarker(
    venueType: String?, category: String = "accessible_toilet", json: String = ""
  ) throws -> Marker {
    var fields = [
      #""id":1"#, #""version":1"#, #""lat":31.2"#, #""lng":121.4"#,
      #""category":"\#(category)""#, #""title":"元数据""#, #""contentLanguage":"zh""#,
    ]
    if let venueType { fields.append(#""venueType":"\#(venueType)""#) }
    if !json.isEmpty { fields.append(json) }
    return try JSONDecoder().decode(
      Marker.self, from: Data("{\(fields.joined(separator: ","))}".utf8))
  }

  private func decodeMarker(json: String) throws -> Marker {
    try decodeMarker(venueType: nil, json: json)
  }

  private func draft(original: Marker? = nil) -> ContributionDraft {
    var value = ContributionDraft(
      owner: "a", origin: "https://i.example.invalid",
      point: GeoPoint(latitude: 31.2, longitude: 121.4)!, language: "en", marker: original)
    value.fields = ContributionStore.normalized(value.fields)
    return value
  }

  private func json(_ draft: ContributionDraft) throws -> [String: Any] {
    try #require(JSONSerialization.jsonObject(with: draft.encodedRequest()) as? [String: Any])
  }

  private func isoDate(_ value: String) -> TimeInterval {
    ISO8601DateFormatter().date(from: value)!.timeIntervalSince1970
  }
}

/// A minimal account fixture for the real-store update regression: it serves an
/// original marker with no venue so the store's edit path can be exercised.
private actor MetadataAccountFixture: AccountServing {
  nonisolated let baseURL = URL(string: "https://metadata.example.invalid")
  func send(_ request: AccountRequest) async throws -> Data {
    if request.path == "api/me" {
      return Data(#"{"code":0,"data":{"publicId":"metadata"}}"#.utf8)
    }
    if request.path.contains("/me/") { return Data("[]".utf8) }
    if request.path == "api/markers/55" {
      return Data(
        #"{"id":55,"version":1,"lat":31.2,"lng":121.4,"category":"accessible_toilet","title":"Original","description":"Before","openTimeStart":"09:00","openTimeEnd":"17:00","contentLanguage":"en","reviewStatus":"PENDING"}"#
          .utf8)
    }
    throw AccountFailure(status: 404)
  }
}
