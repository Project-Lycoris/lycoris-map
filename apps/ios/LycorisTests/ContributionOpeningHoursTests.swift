import Foundation
import Testing

@testable import Lycoris

struct ContributionOpeningHoursTests {
  private func fields(start: String, end: String) -> ContributionFields {
    var fields = ContributionFields(language: "en")
    fields.title = "Opening hours fixture"
    fields.openTimeStart = start
    fields.openTimeEnd = end
    return fields
  }

  private func draft(marker: Marker? = nil) -> ContributionDraft {
    ContributionDraft(
      owner: "hours-test", origin: "https://hours.example.invalid",
      point: GeoPoint(latitude: 31.2, longitude: 121.4)!, language: "en", marker: marker)
  }

  @Test func existingClockValuesInferTheClosingDay() {
    let overnight = fields(start: "10:00", end: "02:00")
    #expect(overnight.closingDayOverride == nil)
    #expect(overnight.closesNextDay)
    #expect(overnight.openingHoursMatchClosingDay)
    #expect(overnight.valid)

    let sameDay = fields(start: "10:00", end: "18:00")
    #expect(!sameDay.closesNextDay)
    #expect(sameDay.openingHoursMatchClosingDay)
    #expect(sameDay.valid)
    #expect(fields(start: "", end: "").valid)
    #expect(!fields(start: "10:00", end: "").valid)
    #expect(!fields(start: "24:00", end: "02:00").valid)
  }

  @Test func explicitClosingDayMustFitTheSharedDailyWindow() {
    var overnight = fields(start: "10:00", end: "02:00")
    overnight.closingDayOverride = true
    #expect(overnight.closesNextDay)
    #expect(overnight.valid)
    overnight.closingDayOverride = false
    #expect(!overnight.closesNextDay)
    #expect(!overnight.openingHoursMatchClosingDay)
    #expect(!overnight.valid)

    var sameDay = fields(start: "10:00", end: "18:00")
    sameDay.closingDayOverride = false
    #expect(sameDay.valid)
    sameDay.closingDayOverride = true
    #expect(sameDay.closesNextDay)
    #expect(!sameDay.openingHoursMatchClosingDay)
    #expect(!sameDay.valid, "A 32-hour interval cannot be represented by two daily clock times")
  }

  @Test func matchingTimesRetainTheTwentyFourHourContract() {
    for nextDay in [false, true] {
      var allDay = fields(start: "10:00", end: "10:00")
      allDay.closingDayOverride = nextDay
      #expect(allDay.openingHoursMatchClosingDay)
      #expect(allDay.valid)
    }
  }

  @Test func legacyDraftWithoutTheClosingDayKeyDecodesAsOvernight() throws {
    let legacy = #"{"id":"5F7E45DF-C662-449B-A981-4D1AB4E94995","owner":"hours-test","origin":"https://hours.example.invalid","point":{"latitude":31.2,"longitude":121.4},"fields":{"title":"Legacy","category":"accessible_toilet","description":"","openTimeStart":"10:00","openTimeEnd":"02:00","language":"en"},"phase":"draft","photoRejected":false}"#
    let decoded = try JSONDecoder().decode(ContributionDraft.self, from: Data(legacy.utf8))
    #expect(decoded.fields.closingDayOverride == nil)
    #expect(decoded.fields.closesNextDay)
    #expect(decoded.canSubmit)
    #expect(decoded.validCheckpoint)
  }

  @Test func draftRoundTripPreservesExplicitClosingDayIncludingUnfinishedInput() throws {
    for nextDay in [false, true] {
      var value = draft()
      value.fields = fields(start: "10:00", end: "02:00")
      value.fields.closingDayOverride = nextDay
      let decoded = try JSONDecoder().decode(
        ContributionDraft.self, from: JSONEncoder().encode(value))
      #expect(decoded == value)
      #expect(decoded.fields.closingDayOverride == nextDay)
      #expect(decoded.fields.closesNextDay == nextDay)
      #expect(decoded.canSubmit == nextDay)
    }
  }

  @Test func nextDaySelectionNeverAddsAFieldToTheApiPayload() throws {
    var value = draft()
    value.fields = fields(start: "10:00", end: "02:00")
    value.fields.closingDayOverride = true
    let body = try #require(
      JSONSerialization.jsonObject(with: value.encodedRequest()) as? [String: Any])
    #expect(body["openTimeStart"] as? String == "10:00")
    #expect(body["openTimeEnd"] as? String == "02:00")
    #expect(body["closingDayOverride"] == nil)
    #expect(body["closesNextDay"] == nil)
    #expect(body["hoursTimezone"] == nil)
  }

  @Test func selectingTheInferredDayAloneDoesNotCreateAnEditProposal() throws {
    let marker = try JSONDecoder().decode(
      Marker.self,
      from: Data(
        #"{"id":55,"version":1,"lat":31.2,"lng":121.4,"category":"accessible_toilet","title":"Existing overnight hours","description":"","openTimeStart":"10:00","openTimeEnd":"02:00","contentLanguage":"en"}"#
          .utf8))
    var edit = draft(marker: marker)
    #expect(edit.fields.closesNextDay)
    #expect(!edit.hasChanges)
    edit.fields.closingDayOverride = true
    #expect(edit.fields.valid)
    #expect(!edit.hasChanges)
    #expect(!edit.canSubmit)
    edit.fields.title = "Updated title"
    #expect(edit.hasChanges)
    #expect(edit.canSubmit)
  }

  @Test func exampleOvernightIntervalUsesThePlaceClockAcrossMidnight() throws {
    let cases: [(String, OpeningStatus)] = [
      ("2026-09-20T01:59:59Z", .closed),
      ("2026-09-20T02:00:00Z", .open),
      ("2026-09-20T16:00:00Z", .open),
      ("2026-09-20T17:30:00Z", .closingSoon),
      ("2026-09-20T18:00:00Z", .closed),
    ]
    for (iso, expected) in cases {
      let now = try #require(ISO8601DateFormatter().date(from: iso))
      #expect(
        OpeningStatusEngine.status(
          start: "10:00", end: "02:00", timeZone: "Asia/Shanghai", now: now) == expected,
        Comment(rawValue: iso))
    }
  }

  @Test func nextDayLabelAndTimeFormatAreLocalized() {
    #expect(
      String(appLocalized: "Closes the next day", language: .chinese, table: "OpeningHours")
        == "次日结束营业")
    #expect(
      String(appLocalized: "%@–%@ (next day)", language: .english, table: "OpeningHours")
        == "%@–%@ (next day)")
    #expect(
      String(appLocalized: "%@–%@ (next day)", language: .chinese, table: "OpeningHours")
        == "%@–次日 %@")
  }
}
