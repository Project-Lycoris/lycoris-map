import Foundation
import Testing

@testable import Lycoris

@MainActor struct ContributionTests {
  @Test func englishClientAndRestoredEnglishDraftSubmitChineseContent() throws {
    var draft = ContributionDraft(
      owner: "synthetic", origin: "https://example.test",
      point: GeoPoint(latitude: 31.2, longitude: 121.5)!, language: "en")
    #expect(draft.fields.language == "zh")
    draft.fields.language = "en"  // A journal created before the language policy changed.
    draft.fields.title = "中文点位"
    let body = try #require(
      JSONSerialization.jsonObject(with: draft.encodedRequest()) as? [String: Any])
    #expect(body["language"] as? String == "zh")
  }

  @Test func multiplePhotosKeepOrderAndResumeWithoutRepeatingCreation() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer {
      store.setActive(false)
      try? journal.clear()
    }
    try fill(store)
    try store.addPhoto(Data(repeating: 1, count: 12))
    try store.addPhoto(Data(repeating: 2, count: 19))
    let ids = try #require(store.draft).remainingPhotos.map { $0.id.uuidString }
    #expect(ids.count == 2)
    await api.lose("complete")
    store.submit()
    try await settle { store.message != nil }
    store.setActive(false)
    let (_, resumed, _) = await setup(api, journal: journal)
    defer { resumed.setActive(false) }
    try await settle { resumed.draft?.phase == .complete }
    #expect(await api.creates == 1)
    #expect(await api.completions == 2)
    #expect(await api.photoRequests == ids)
    #expect(resumed.drafts.isEmpty)
  }

  @Test func keepsSeveralDraftsAndDeletingOnePreservesAnother() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try fill(store)
    let first = try #require(store.draft?.id)
    try store.begin(at: GeoPoint(latitude: 32, longitude: 120)!)
    let second = try #require(store.draft?.id)
    #expect(first != second && store.drafts.count == 2)
    try store.openDraft(first)
    #expect(store.draft?.fields.title == "I5 fixture")
    try store.deleteDraft(first)
    #expect(store.drafts.map(\.id) == [second])
  }

  private func setup(_ api: ContributionFixture, journal: ContributionJournal? = nil) async -> (
    AccountStore, ContributionStore, ContributionJournal
  ) {
    let journal =
      journal
      ?? ContributionJournal(
        directory: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
    let account = AccountStore(api: api)
    let store = ContributionStore(journal: journal)
    store.connect(account, monitorNetwork: false)
    await account.restore()
    await account.loadLibrary()
    store.synchronize()
    return (account, store, journal)
  }
  private func settle(_ condition: () -> Bool) async throws {
    for _ in 0..<300 {
      if condition() { return }
      try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("Contribution did not settle")
  }
  private func fill(_ store: ContributionStore) throws {
    try store.begin(at: GeoPoint(latitude: 31.2, longitude: 121.4)!)
    var fields = try #require(store.draft?.fields)
    fields.title = "I5 fixture"
    store.update(fields)
  }

  @Test func coldEditInitializesWithoutOpeningCreateOrSynchronizingFirst() async throws {
    let api = ContributionFixture()
    let journal = ContributionJournal(
      directory: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
    defer { try? journal.clear() }
    let account = AccountStore(api: api)
    let store = ContributionStore(journal: journal)
    store.connect(account, monitorNetwork: false)
    await account.restore()
    try await store.edit(55)
    #expect(store.draft?.original?.id == 55 && store.draft?.fields.title == "Original")
  }

  @Test func editWaitsForForegroundIdentityRestore() async throws {
    let api = ContributionFixture()
    let (account, store, journal) = await setup(api)
    defer { try? journal.clear() }
    await api.holdIdentity()
    let restoring = Task { await account.restore() }
    for _ in 0..<200 {
      if await api.identityHeld { break }
      try await Task.sleep(for: .milliseconds(5))
    }
    #expect(await api.identityHeld)
    let editing = Task { try await store.edit(55) }
    await Task.yield()
    #expect(store.draft == nil)
    await api.releaseIdentity()
    await restoring.value
    try await editing.value
    #expect(store.draft?.fields.title == "Original")
  }

  @Test func closingEditWhileWaitingDoesNotCreateADraftLater() async throws {
    let api = ContributionFixture()
    let (account, store, journal) = await setup(api)
    defer { try? journal.clear() }
    await api.holdIdentity()
    let restoring = Task { await account.restore() }
    for _ in 0..<200 {
      if await api.identityHeld { break }
      try await Task.sleep(for: .milliseconds(5))
    }
    #expect(await api.identityHeld)
    let editing = Task { try await store.edit(55) }
    try await Task.sleep(for: .milliseconds(20))
    editing.cancel()
    do {
      try await editing.value
      Issue.record("Dismissed editing must cancel")
    } catch { #expect(error is CancellationError) }
    await api.releaseIdentity()
    await restoring.value
    #expect(store.draft == nil)
    #expect(try journal.load() == nil)
  }

  @Test func closingEditDuringDetailReadDoesNotSaveADraft() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    await api.holdDetail()
    let editing = Task { try await store.edit(55) }
    for _ in 0..<200 {
      if await api.detailHeld { break }
      try await Task.sleep(for: .milliseconds(5))
    }
    #expect(await api.detailHeld)
    editing.cancel()
    await api.releaseDetail()
    do {
      try await editing.value
      Issue.record("A cancelled detail read must not create an edit")
    } catch { #expect(error is CancellationError) }
    #expect(store.draft == nil)
    #expect(try journal.load() == nil)
  }

  @Test func accountChangeWhileEditWaitsCannotCreateAnotherOwnersDraft() async throws {
    let api = ContributionFixture()
    let (account, store, journal) = await setup(api)
    defer { try? journal.clear() }
    await api.holdIdentity()
    let restoring = Task { await account.restore() }
    for _ in 0..<200 {
      if await api.identityHeld { break }
      try await Task.sleep(for: .milliseconds(5))
    }
    #expect(await api.identityHeld)
    let editing = Task { try await store.edit(55) }
    try await Task.sleep(for: .milliseconds(20))
    await api.switchOwner("b")
    await api.releaseIdentity()
    await restoring.value
    do {
      try await editing.value
      Issue.record("Account changes must cancel the pending edit")
    } catch { #expect(error is CancellationError) }
    #expect(account.user?.publicId == "b" && store.draft == nil)
  }

  @Test func lostCreateReceiptRecoversFrozenUUIDAndPayloadAfterRelaunch() async throws {
    let api = ContributionFixture()
    await api.lose("create")
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try fill(store)
    store.submit()
    try await settle { store.message != nil }
    let frozen = try #require(store.draft)
    #expect(frozen.phase == .creating)
    var edited = frozen.fields
    edited.title = "Must not replace frozen payload"
    store.update(edited)
    #expect(store.draft?.fields == frozen.fields)
    store.setActive(false)
    let (_, recovered, _) = await setup(api, journal: journal)
    try await settle { recovered.draft?.phase == .complete }
    #expect(await api.creates == 1)
    let bodies = await api.createBodies
    #expect(bodies.count == 2 && bodies[0] == bodies[1])
    #expect(recovered.draft?.id == frozen.id)
    recovered.setActive(false)
  }

  @Test func lostChunkAndCompletionResumeSameBytesAcrossProcesses() async throws {
    let api = ContributionFixture()
    await api.lose("chunk")
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try fill(store)
    let bytes = Data((0..<400_000).map { UInt8($0 % 251) })
    try store.choosePhoto(bytes)
    let photoID = store.draft?.photoID
    store.submit()
    try await settle { store.message != nil }
    #expect(store.draft?.markerID == 55 && store.draft?.phase == .uploading)
    store.setActive(false)
    await api.lose("complete")
    let (_, resumed, _) = await setup(api, journal: journal)
    try await settle { resumed.message != nil }
    #expect(await api.uploaded == bytes)
    #expect(resumed.draft?.photoID == photoID)
    resumed.setActive(false)
    journal.removePhoto(photoID)
    let (_, completed, _) = await setup(api, journal: journal)
    try await settle { completed.draft?.phase == .complete }
    #expect(await api.creates == 1)
    #expect(await api.completions == 1)
    #expect(await api.chunkOffsets == [0, 262_144])
    #expect(!FileManager.default.fileExists(atPath: journal.photoURL(photoID!).path))
    completed.setActive(false)
  }

  @Test func laterRejectionCannotUnfreezeAnUncertainCreate() async throws {
    let api = ContributionFixture()
    await api.lose("create")
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try fill(store)
    store.submit()
    try await settle { store.message != nil }
    let frozen = store.draft?.requestBody
    store.setActive(false)
    await api.rejectCreate(400)
    let (_, recovered, _) = await setup(api, journal: journal)
    try await settle { recovered.message != nil }
    #expect(recovered.draft?.phase == .creating)
    #expect(recovered.draft?.requestBody == frozen)
    recovered.setActive(false)
  }

  @Test func delayedEditReadCannotOverwriteANewerDraft() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    await api.holdDetail()
    let loading = Task { try? await store.edit(55) }
    for _ in 0..<100 {
      if await api.detailHeld { break }
      try await Task.sleep(for: .milliseconds(5))
    }
    try fill(store)
    let id = store.draft?.id
    await api.releaseDetail()
    await loading.value
    #expect(store.draft?.id == id && store.draft?.original == nil)
  }

  @Test func ambiguousEditNeverAutomaticallyReplaysIncludingAfterRestart() async throws {
    let api = ContributionFixture()
    await api.lose("edit")
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try await store.edit(55)
    var fields = try #require(store.draft?.fields)
    fields.title = "Edited title"
    fields.description = ""
    fields.openTimeStart = ""
    fields.openTimeEnd = ""
    store.update(fields)
    store.submit()
    try await settle { store.draft?.phase == .uncertainEdit }
    store.retry()
    store.setActive(false)
    let (_, resumed, _) = await setup(api, journal: journal)
    #expect(resumed.draft?.phase == .uncertainEdit)
    #expect(await api.edits == 1)
    let body = try #require(await api.editBody)
    let json = try #require(JSONSerialization.jsonObject(with: body) as? [String: Any])
    #expect(json["language"] as? String == "zh")
    #expect(json["description"] as? String == "" && json["openTimeStart"] as? String == "")
    #expect(json["lat"] == nil && json["isPublic"] == nil && json["isActive"] == nil)
    try resumed.resendEdit()
    try await settle { resumed.draft?.phase == .complete }
    #expect(await api.edits == 2)
    resumed.setActive(false)
  }

  @Test func photoOnlyEditSkipsTextProposal() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try await store.edit(55)
    try store.choosePhoto(Data(repeating: 7, count: 262_145))
    store.submit()
    try await settle { store.draft?.phase == .complete }
    #expect(await api.edits == 0)
    #expect(await api.creates == 0)
    #expect(await api.completions == 1)
    store.setActive(false)
  }

  @Test func editPreflightOutageKeepsAnExplicitSubmitWithoutClaimingAutomaticResume() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try await store.edit(55)
    var fields = try #require(store.draft?.fields)
    fields.title = "New title"
    store.update(fields)
    await api.lose("preflight")
    store.submit()
    try await settle { store.message != nil }
    #expect(store.draft?.editable == true)
    #expect(store.message == AccountFailure(status: 0).message)
    #expect(await api.edits == 0)
    store.submit()
    try await settle { store.draft?.phase == .complete }
    #expect(await api.edits == 1)
    store.setActive(false)
  }

  @Test func malformedDurableCheckpointIsRejectedBeforeAnyRequest() async throws {
    let journal = ContributionJournal(
      directory: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString))
    defer { try? journal.clear() }
    var value = ContributionDraft(
      owner: "a", origin: "https://i5.example.invalid", point: GeoPoint(latitude: 1, longitude: 1)!,
      language: "en")
    value.phase = .uploading
    try journal.save(value)
    #expect(throws: ContributionFailure.storage) { try journal.load() }
    let api = ContributionFixture()
    let (_, store, _) = await setup(api, journal: journal)
    #expect(store.draft == nil && store.message != nil)
    #expect(await api.creates == 0)
  }

  @Test func logoutHidesJournalAndSwitchCannotSubmitOldWork() async throws {
    let api = ContributionFixture()
    await api.lose("chunk")
    let (account, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try fill(store)
    try store.choosePhoto(Data(repeating: 8, count: 300_000))
    store.submit()
    try await settle { store.message != nil }
    let writes = await api.chunkOffsets.count
    await account.logout()
    #expect(store.draft == nil && store.photoPreview == nil)
    #expect(try journal.load() != nil)
    await api.switchOwner("b")
    await account.restore()
    store.synchronize()
    store.resume()
    #expect(store.draft == nil && account.user?.publicId == "b")
    #expect(await api.chunkOffsets.count == writes)
    store.setActive(false)
  }

  @Test func persistedDifferentOriginOrOwnerNeverResumes() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try fill(store)
    store.setActive(false)
    await api.switchOwner("b")
    let (_, other, _) = await setup(api, journal: journal)
    #expect(other.draft == nil)
    #expect(try journal.load() != nil)
    #expect(await api.creates == 0)
  }

  @Test func missingBytesPauseWithoutReplacingMarkerSubmission() async throws {
    let api = ContributionFixture()
    let (_, store, journal) = await setup(api)
    defer { try? journal.clear() }
    try fill(store)
    try store.choosePhoto(Data(repeating: 1, count: 10))
    var saved = try #require(store.draft)
    saved.markerID = 55
    saved.phase = .uploading
    try journal.save(saved)
    journal.removePhoto(saved.photoID)
    store.setActive(false)
    let (_, resumed, _) = await setup(api, journal: journal)
    try await settle { resumed.draft?.photoRejected == true }
    #expect(resumed.draft?.markerID == 55)
    #expect(await api.creates == 0)
    #expect(await api.completions == 0)
    try resumed.choosePhoto(Data(repeating: 2, count: 10))
    try await settle { resumed.draft?.phase == .complete }
    #expect(await api.creates == 0)
    #expect(await api.completions == 1)
    resumed.setActive(false)
  }

  @Test func durableWriteFailurePreventsNetworkAndReceiptValidationRejectsCorruption() async throws
  {
    let api = ContributionFixture()
    let file = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
    try Data("not a directory".utf8).write(to: file)
    defer { try? FileManager.default.removeItem(at: file) }
    let (_, store, _) = await setup(api, journal: ContributionJournal(directory: file))
    #expect(throws: (any Error).self) { try fill(store) }
    #expect(await api.creates == 0)
    var draft = ContributionDraft(
      owner: "a", origin: "fixture", point: GeoPoint(latitude: 0, longitude: 0)!, language: "en")
    draft.markerID = 55
    draft.photoSize = 300_000
    let invalid = UploadReceipt(
      uploadId: UUID().uuidString, markerId: 55, totalBytes: 300_000,
      receivedBytes: 100, chunkSize: 262_144, status: "COMPLETED")
    #expect(throws: ContributionFailure.invalidReceipt) { try invalid.validate(for: draft) }
  }
}

private actor ContributionFixture: AccountServing {
  nonisolated let baseURL = URL(string: "https://i5.example.invalid")
  private var owner: String? = "a"
  private var lost: String?
  private var createRejection: Int?
  private var holdsDetail = false
  private var holdsIdentity = false
  private var identityContinuation: CheckedContinuation<Void, Never>?
  var identityHeld: Bool { identityContinuation != nil }
  func holdIdentity() { holdsIdentity = true }
  func releaseIdentity() {
    identityContinuation?.resume()
    identityContinuation = nil
  }
  private var detailContinuation: CheckedContinuation<Void, Never>?
  var detailHeld: Bool { detailContinuation != nil }
  private var ids = Set<String>()
  private var uploadID = UUID().uuidString
  private var photoRequestID: String?
  var photoRequests: [String] = []
  private var total = 0
  private var completed = false
  var creates = 0
  var edits = 0
  var completions = 0
  var createBodies: [Data] = []
  var editBody: Data?
  var uploaded = Data()
  var chunkOffsets: [Int] = []
  func lose(_ value: String) { lost = value }
  func rejectCreate(_ status: Int?) { createRejection = status }
  func holdDetail() { holdsDetail = true }
  func releaseDetail() {
    detailContinuation?.resume()
    detailContinuation = nil
    holdsDetail = false
  }
  func switchOwner(_ value: String) { owner = value }
  private func loseIfNeeded(_ value: String) throws {
    if lost == value {
      lost = nil
      throw URLError(.networkConnectionLost)
    }
  }
  func send(_ request: AccountRequest) async throws -> Data {
    if request.path == "api/logout" {
      owner = nil
      return Data()
    }
    if request.path == "api/me", holdsIdentity {
      holdsIdentity = false
      await withCheckedContinuation { identityContinuation = $0 }
    }
    guard let owner else { throw AccountFailure(status: 401) }
    if request.path == "api/me" {
      try loseIfNeeded("preflight")
      return Data("{\"code\":0,\"data\":{\"publicId\":\"\(owner)\"}}".utf8)
    }
    if request.path.contains("/me/") { return Data("[]".utf8) }
    let marker = Data(
      #"{"id":55,"version":1,"lat":31.2,"lng":121.4,"category":"accessible_toilet","title":"Original","description":"Before","openTimeStart":"09:00","openTimeEnd":"17:00","contentLanguage":"zh","reviewStatus":"PENDING"}"#
        .utf8)
    if request.path == "api/markers", request.method == "POST" {
      if let createRejection { throw AccountFailure(status: createRejection) }
      createBodies.append(request.body!)
      let json = try JSONSerialization.jsonObject(with: request.body!) as! [String: Any]
      if ids.insert(json["clientRequestId"] as! String).inserted { creates += 1 }
      try loseIfNeeded("create")
      return marker
    }
    if request.path == "api/markers/55" {
      if holdsDetail && request.method == "GET" {
        await withCheckedContinuation { detailContinuation = $0 }
      }
      if request.method == "PATCH" {
        edits += 1
        editBody = request.body
        try loseIfNeeded("edit")
      }
      return marker
    }
    if request.path.hasSuffix("image-uploads") {
      let json = try JSONSerialization.jsonObject(with: request.body!) as! [String: Any]
      let photoID = json["clientRequestId"] as! String
      if photoID != photoRequestID {
        photoRequestID = photoID
        photoRequests.append(photoID)
        uploadID = UUID().uuidString
        uploaded = Data()
        completed = false
      }
      total = json["totalBytes"] as! Int
    } else if request.path.contains("/chunks/") {
      let offset = Int(request.path.split(separator: "/").last!)!
      chunkOffsets.append(offset)
      guard uploaded.count == offset else { throw AccountFailure(status: 409) }
      uploaded.append(request.body!)
      try loseIfNeeded("chunk")
    } else if request.path.hasSuffix("complete") {
      if !completed { completions += 1 }
      completed = true
      try loseIfNeeded("complete")
    }
    return try JSONEncoder().encode(
      UploadReceipt(
        uploadId: uploadID, markerId: 55,
        totalBytes: total, receivedBytes: uploaded.count, chunkSize: 262_144,
        status: completed ? "COMPLETED" : "UPLOADING"))
  }
}
