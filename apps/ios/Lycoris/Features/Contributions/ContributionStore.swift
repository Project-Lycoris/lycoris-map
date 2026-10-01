import Foundation
import Network
import Observation

@MainActor @Observable
final class ContributionStore {
  private(set) var draft: ContributionDraft?
  private(set) var isWorking = false
  private(set) var message: String?
  private(set) var photoPreview: Data?
  private let journal: ContributionJournal
  private var account: AccountStore?
  private var boundEpoch: UUID?
  private var task: Task<Void, Never>?
  private var generation = UUID()
  private var active = true
  private var attempts = 0
  private var paused = false
  private var monitor: NWPathMonitor?

  init(journal: ContributionJournal = ContributionJournal()) { self.journal = journal }

  func connect(_ account: AccountStore, monitorNetwork: Bool = true) {
    guard self.account == nil else { return }
    self.account = account
    account.onPrivateDataInvalidated = { [weak self] in self?.invalidate() }
    if monitorNetwork {
      let monitor = NWPathMonitor()
      monitor.pathUpdateHandler = { [weak self] path in
        guard path.status == .satisfied else { return }
        Task { @MainActor [weak self] in self?.resume() }
      }
      monitor.start(queue: DispatchQueue(label: "lycoris.photo-network"))
      self.monitor = monitor
    }
  }

  func synchronize() {
    guard let account, account.hasVerifiedIdentity else { return }
    guard let owner = account.user?.publicId, let origin = account.baseURL?.absoluteString else {
      invalidate(force: true)
      return
    }
    guard boundEpoch != account.epoch else {
      resume()
      return
    }
    boundEpoch = account.epoch
    do {
      if var saved = try journal.load() {
        guard saved.owner == owner, saved.origin == origin else {
          try journal.clear()
          return
        }
        if saved.phase == .editing {
          saved.phase = .uncertainEdit
          try journal.save(saved)
        }
        draft = saved
        if saved.photoID != nil {
          photoPreview = try? journal.photo(saved)
          if photoPreview == nil && saved.phase == .draft {
            saved.photoRejected = true
            draft = saved
            try journal.save(saved)
            message = String(
              appLocalized: "The saved photo is unavailable. Please choose it again.")
          }
        }
        resume()
      }
    } catch { message = String(appLocalized: "Could not read the saved contribution.") }
  }

  func begin(at point: GeoPoint) throws {
    guard let account, let owner = account.user?.publicId,
      let origin = account.baseURL?.absoluteString, boundEpoch == account.epoch
    else { throw AccountFailure(status: 401) }
    if draft?.phase == .complete { try discard() }
    guard draft == nil else { return }
    var value = ContributionDraft(
      owner: owner, origin: origin, point: point, language: "zh")
    value.fields = Self.normalized(value.fields)
    try journal.save(value)
    draft = value
    message = nil
  }

  func edit(_ id: Int64) async throws {
    synchronize()
    guard let account, let owner = account.user?.publicId, let token = boundEpoch,
      let origin = account.baseURL?.absoluteString
    else { throw AccountFailure(status: 401) }
    if draft?.phase == .complete { try discard() }
    guard draft == nil else { return }
    let generation = self.generation
    let data = try await account.contributionRequest(
      AccountRequest(path: "api/markers/\(id)", query: ["lang": "zh"]), owner: owner,
      token: token, waitForAccount: true)
    let marker = try JSONDecoder().decode(Marker.self, from: data)
    guard generation == self.generation, draft == nil, token == boundEpoch,
      owner == account.user?.publicId
    else { throw CancellationError() }
    guard marker.id == id, let point = marker.point else {
      throw ContributionFailure.invalidReceipt
    }
    let value = ContributionDraft(
      owner: owner, origin: origin, point: point,
      language: "zh", marker: marker)
    try journal.save(value)
    draft = value
    message = nil
  }

  func update(_ fields: ContributionFields) {
    guard var value = draft, value.editable, !isWorking else { return }
    value.fields = Self.normalized(fields)
    do {
      try journal.save(value)
      draft = value
      message = nil
    } catch { message = String(appLocalized: "Could not save the contribution on this device.") }
  }

  /// Keeps the venue tag consistent with the chosen category: a non-toilet
  /// never keeps a tag that could be sent for the wrong type. It never invents
  /// an `other` value for an existing draft whose venue is absent, so an old
  /// draft edited only in text/time cannot overwrite a server-side
  /// reclassification. A brand-new toilet already starts at `other` from
  /// `ContributionFields(language:)`; an explicit category switch back to a
  /// toilet is handled by `switchingCategory`.
  nonisolated static func normalized(_ fields: ContributionFields) -> ContributionFields {
    var value = fields
    if value.category != .toilet {
      value.venueType = nil
      value.unknownVenueType = nil
    }
    return value
  }

  /// Applies the venue rule for an explicit category change: leaving the toilet
  /// category clears any tag, and returning to it starts at the `other` default
  /// without a stale value.
  nonisolated static func switchingCategory(
    _ fields: ContributionFields, from previous: PlaceCategory
  ) -> ContributionFields {
    var value = normalized(fields)
    if previous != .toilet, value.category == .toilet,
      value.venueType == nil, value.unknownVenueType == nil
    {
      value.venueType = .other
    }
    return value
  }

  func choosePhoto(_ encoded: Data) throws {
    guard var value = draft, !isWorking, value.editable || value.photoRejected else { return }
    let old = value.photoID
    let id = UUID()
    let info = try journal.savePhoto(encoded, id: id)
    value.photoID = id
    value.photoSize = info.size
    value.photoHash = info.hash
    value.upload = nil
    value.photoRejected = false
    do { try journal.save(value) } catch {
      journal.removePhoto(id)
      throw error
    }
    draft = value
    photoPreview = encoded
    message = nil
    paused = false
    journal.removePhoto(old)
    if value.phase == .uploading { resume() }
  }

  func move(to point: GeoPoint) throws {
    guard var value = draft, value.original == nil, value.editable, !isWorking else { return }
    value.point = point
    try checkpoint(value)
  }

  func removePhoto() throws {
    guard var value = draft, value.editable, !isWorking else { return }
    let old = value.photoID
    value.photoID = nil
    value.photoSize = nil
    value.photoHash = nil
    value.upload = nil
    value.photoRejected = false
    try journal.save(value)
    draft = value
    photoPreview = nil
    journal.removePhoto(old)
  }

  func submit() {
    guard let draft, draft.editable, draft.canSubmit else { return }
    paused = false
    attempts = 0
    start()
  }

  func retry() {
    paused = false
    attempts = 0
    resume()
  }

  /// The caller must explain the duplicate-proposal risk before this explicit action.
  func resendEdit() throws {
    guard var value = draft, value.phase == .uncertainEdit else { return }
    value.phase = .draft
    try journal.save(value)
    draft = value
    submit()
  }

  func discard() throws {
    stop()
    try journal.clear()
    draft = nil
    photoPreview = nil
    message = nil
    paused = false
    attempts = 0
  }

  func setActive(_ active: Bool) {
    self.active = active
    if active { resume() } else { stop() }
  }

  func resume() {
    guard let draft, [.creating, .uploading].contains(draft.phase),
      !draft.photoRejected, !paused
    else { return }
    start()
  }

  private func invalidate(force: Bool = false) {
    // Initial /me verification must not erase the previous process's journal.
    guard force || boundEpoch != nil else { return }
    stop()
    boundEpoch = nil
    draft = nil
    photoPreview = nil
    message = nil
    paused = false
    do { try journal.clear() } catch {
      message = String(appLocalized: "Could not clear the saved contribution.")
    }
  }

  private func stop() {
    if var value = draft, value.phase == .editing {
      value.phase = .uncertainEdit
      draft = value
      try? journal.save(value)
    }
    task?.cancel()
    task = nil
    generation = UUID()
    isWorking = false
  }

  private func start() {
    guard active, task == nil, draft != nil, let account,
      boundEpoch == account.epoch, account.user?.publicId == draft?.owner
    else { return }
    let token = generation
    isWorking = true
    message = nil
    task = Task { [weak self] in
      guard let self else { return }
      var shouldRetry = false
      do {
        try await self.run(token)
        self.attempts = 0
      } catch {
        guard token == self.generation else { return }
        shouldRetry = self.handle(error)
      }
      guard token == self.generation else { return }
      self.isWorking = false
      if shouldRetry && self.active {
        self.attempts += 1
        let delay = min(60, pow(2, Double(min(self.attempts, 6))))
        do { try await Task.sleep(for: .seconds(delay)) } catch { return }
      }
      guard token == self.generation else { return }
      self.task = nil
      if shouldRetry { self.resume() }
    }
  }

  private func request(
    _ input: AccountRequest, token: UUID,
    beforeSend: () throws -> Void = {}
  ) async throws -> Data {
    try check(token)
    guard let account, let draft, let epoch = boundEpoch else { throw CancellationError() }
    return try await account.contributionRequest(
      input, owner: draft.owner, token: epoch, beforeSend: beforeSend)
  }

  private func check(_ token: UUID) throws {
    try Task.checkCancellation()
    guard token == generation, let draft, let account,
      account.epoch == boundEpoch, account.user?.publicId == draft.owner,
      account.baseURL?.absoluteString == draft.origin
    else { throw CancellationError() }
  }

  private func checkpoint(_ value: ContributionDraft) throws {
    try journal.save(value)
    draft = value
  }

  private func run(_ token: UUID) async throws {
    guard var value = draft else { return }
    if value.phase == .draft || value.phase == .creating {
      guard value.canSubmit else { throw ContributionFailure.invalidFields }
      if value.requestBody == nil { value.requestBody = try value.encodedRequest() }
      if value.original == nil {
        value.phase = .creating
        try checkpoint(value)
      }
      if value.original == nil || value.hasChanges {
        let editing = value.original != nil
        let data = try await request(
          AccountRequest(
            path: editing ? "api/markers/\(value.original!.id)" : "api/markers",
            method: editing ? "PATCH" : "POST", body: value.requestBody,
            query: ["lang": value.fields.language]), token: token,
          beforeSend: {
            if editing {
              value.phase = .editing
              try self.checkpoint(value)
            }
          })
        try check(token)
        let marker = try JSONDecoder().decode(Marker.self, from: data)
        guard marker.id > 0, marker.point != nil,
          value.original == nil || marker.id == value.original?.id
        else {
          throw ContributionFailure.invalidReceipt
        }
        value.markerID = marker.id
      }
      value.phase = value.photoID == nil ? .complete : .uploading
      try checkpoint(value)
    }
    if value.phase == .uploading {
      let path = "api/markers/\(value.markerID!)/image-uploads"
      let data: Data
      if let receipt = value.upload {
        data = try await request(AccountRequest(path: "\(path)/\(receipt.uploadId)"), token: token)
      } else {
        let body = try JSONSerialization.data(withJSONObject: [
          "clientRequestId": value.photoID!.uuidString, "totalBytes": value.photoSize!,
          "sha256": value.photoHash!,
        ])
        data = try await request(
          AccountRequest(path: path, method: "POST", body: body), token: token)
      }
      try check(token)
      var receipt = try JSONDecoder().decode(UploadReceipt.self, from: data)
      try receipt.validate(for: value)
      value.upload = receipt
      try checkpoint(value)
      // Reconcile completion before touching local bytes: the server can finish
      // after a disconnect, even if the local file is no longer available.
      let bytes = receipt.receivedBytes < receipt.totalBytes ? try journal.photo(value) : Data()
      while !receipt.complete {
        try check(token)
        let base = "\(path)/\(receipt.uploadId)"
        let response: Data
        if receipt.receivedBytes < receipt.totalBytes {
          let end = min(bytes.count, receipt.receivedBytes + receipt.chunkSize)
          response = try await request(
            AccountRequest(
              path: "\(base)/chunks/\(receipt.receivedBytes)", method: "POST",
              body: bytes.subdata(in: receipt.receivedBytes..<end),
              contentType: "application/octet-stream"), token: token)
        } else {
          response = try await request(
            AccountRequest(path: "\(base)/complete", method: "POST"), token: token)
        }
        try check(token)
        let next = try JSONDecoder().decode(UploadReceipt.self, from: response)
        try next.validate(for: value)
        guard next.complete || next.receivedBytes > receipt.receivedBytes else {
          throw ContributionFailure.invalidReceipt
        }
        receipt = next
        value.upload = receipt
        try checkpoint(value)
      }
      value.phase = .complete
      try checkpoint(value)
    }
    if value.phase == .complete {
      account?.reloadLibrary()
      journal.removePhoto(value.photoID)
      photoPreview = nil
    }
  }

  private func handle(_ error: Error) -> Bool {
    guard var value = draft else { return false }
    let status = (error as? AccountFailure)?.status
    if value.phase == .editing {
      // Only an explicit 4xx rejection confirms that no proposal was accepted.
      value.phase =
        status.map { (400..<500).contains($0) && $0 != 408 } == true ? .draft : .uncertainEdit
      if value.phase == .draft { value.requestBody = nil }
      draft = value
      try? checkpoint(value)
      message =
        value.phase == .draft
        ? (error as? AccountFailure)?.message
        : String(
          appLocalized: "The edit could not be confirmed. It may already be awaiting review.")
      return false
    }
    if error as? ContributionFailure == .accountBusy {
      if value.phase == .draft {
        message = String(appLocalized: "Another account action is still running. Please try again.")
        return false
      }
      return true
    }
    if status == 410 || error as? ContributionFailure == .missingPhoto
      || (value.phase == .uploading && [400, 413, 415].contains(status ?? 0))
    {
      value.photoRejected = true
      try? checkpoint(value)
      message = String(
        appLocalized: "Please choose the photo again. Your place submission is saved.")
      return false
    }
    let transient = (error is URLError) || status == 0 || status == 408 || (status ?? 0) >= 500
    if value.phase == .draft {
      message = (error as? AccountFailure)?.message ?? AccountFailure(status: 0).message
      return false
    }
    let conflict = status == 409 && value.phase == .uploading && attempts < 1
    if transient || conflict {
      message = String(
        appLocalized: "Saved on this device. Submission will resume when the connection returns.")
      return true
    }
    paused = true
    message =
      (error as? AccountFailure)?.message
      ?? String(appLocalized: "Could not save or verify the contribution. Please try again.")
    return false
  }
}
