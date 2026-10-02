import Foundation
import Observation

/// Owns identity and private UI data together. Async reads must validate their
/// captured epoch before publishing, even when transport cancellation was requested.
@MainActor @Observable
final class AccountStore {
  private let api: any AccountServing
  private(set) var user: AccountUser?
  private(set) var isChecking = false { didSet { resumeAccountWaiters() } }
  private(set) var isBusy = false { didSet { resumeAccountWaiters() } }
  private(set) var hasChecked = false
  private(set) var hasVerifiedIdentity = false
  private(set) var epoch = UUID()
  var message: String?
  private(set) var bookmarks: [Marker] = []
  private(set) var created: [Marker] = []
  private(set) var libraryLoading = false
  private(set) var bookmarkStatusLoading = false
  private(set) var libraryMessage: String?
  private(set) var avatar: Data?
  private(set) var selectedMarker: Marker?
  private(set) var selectedPhoto: Data?
  private(set) var photoFailed = false
  private(set) var detailState: PlaceStore.LoadState = .idle
  private var libraryGeneration = UUID()
  private var detailGeneration = UUID()
  private var identityGeneration = UUID()
  private var networkGeneration = 0
  private var bookmarkOverrides: [Int64: Bool] = [:]
  private var bookmarkRevision = UUID()
  private var bookmarkWritePending = false
  private var hasLoadedBookmarks = false
  private var bookmarkTask: Task<Void, Never>?
  @ObservationIgnored private var accountWaiters: [UUID: CheckedContinuation<Void, any Error>] = [:]
  private var libraryTask: Task<Void, Never>?
  private var detailTask: Task<Void, Never>?
  @ObservationIgnored var onPrivateDataInvalidated: (() -> Void)?

  var baseURL: URL? { api.baseURL }
  private(set) var language = AppLanguage.current().rawValue

  func updateLanguage(_ value: String) {
    guard language != value else { return }
    language = value
    message = nil
    reloadLibrary()
    if let selectedMarker { select(selectedMarker) }
  }

  init(api: any AccountServing = AccountAPI()) { self.api = api }

  func networkDidRecover() async {
    networkGeneration += 1
    await restore()
  }

  func restore() async {
    guard !isBusy, !isChecking else { return }
    isChecking = true
    let token = epoch
    let generation = identityGeneration
    let network = networkGeneration
    defer {
      isChecking = false
      hasChecked = true
    }
    do {
      let value = try await api.user()
      guard token == epoch, generation == identityGeneration else { return }
      accept(value)
      message = nil
      reloadLibrary()
    } catch {
      guard token == epoch, generation == identityGeneration else { return }
      if error is URLError, network != networkGeneration {
        // Recovery may arrive just before this in-flight read reports failure.
        Task { [weak self] in await self?.restore() }
        return
      }
      if (error as? AccountFailure)?.status == 401 {
        expire()
      } else {
        message = failureMessage(error)
      }
    }
  }

  /// Auth and account writes are serialized. A dismissed sheet cannot cancel a cookie-changing request.
  func authenticate(
    username: String, email: String, password: String, register: Bool, verificationCode: String = ""
  ) async -> Bool {
    guard !isBusy else { return false }
    isBusy = true
    invalidatePrivateData()
    user = nil
    message = nil
    defer {
      isBusy = false
      hasChecked = true
    }
    do {
      var fields = [
        "username": username.trimmingCharacters(in: .whitespacesAndNewlines), "password": password,
      ]
      if register {
        fields["email"] = email.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        fields["verificationCode"] = verificationCode
      }
      _ = try await api.user(.json(register ? "api/register" : "api/login", fields: fields))
      // Verify the cookie, rather than trusting only the login response body.
      let current = try await api.user()
      accept(current)
      reloadLibrary()
      return true
    } catch {
      await reconcile()
      let failure = error as? AccountFailure
      if [40021, 40022, 42931, 42932, 50321].contains(failure?.code ?? 0) {
        message = failureMessage(error)
        if failure?.code == 42931 {
          verificationLockedEmail = email.trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
          verificationLockedUntil = Date().addingTimeInterval(
            Double(failure?.retryAfterSeconds ?? 3600))
        }
      } else if !register && failure?.status == 401 {
        message = String(
          appLocalized: "The email, username, or password is incorrect.", table: "AccountFeedback")
      } else if register && failure?.status == 503 {
        message = String(
          appLocalized:
            "Registration could not be confirmed. Your account may have been created; try logging in."
        )
      } else if register && failure?.status == 400 {
        message = String(
          appLocalized: "Check your details. That username or email may already be in use.")
      } else {
        message = failureMessage(error)
      }
      return false
    }
  }

  private(set) var verificationLockedUntil = Date.distantPast
  private(set) var verificationLockedEmail = ""

  func sendEmailCode(email: String, reset: Bool) async throws {
    guard !isBusy else { throw AccountFailure(status: 429) }
    isBusy = true
    message = nil
    defer { isBusy = false }
    _ = try await api.send(
      .json(
        "api/auth/email-code",
        fields: [
          "email": email.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(),
          "purpose": reset ? "reset_password" : "register",
        ]))
  }

  func resetPassword(email: String, code: String, password: String) async -> Bool {
    guard !isBusy else { return false }
    isBusy = true
    message = nil
    defer { isBusy = false }
    do {
      _ = try await api.send(
        .json(
          "api/auth/reset-password",
          fields: [
            "email": email.trimmingCharacters(in: .whitespacesAndNewlines).lowercased(),
            "verificationCode": code, "newPassword": password,
          ]))
      // The acknowledged reset revoked the session server-side; do not retain
      // private state if a subsequent network read is unavailable.
      expire()
      hasChecked = true
      message = String(appLocalized: "Password reset. Please log in with your new password.")
      return true
    } catch {
      let failure = error as? AccountFailure
      if failure?.code == 42931 {
        verificationLockedEmail = email.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        verificationLockedUntil = Date().addingTimeInterval(
          Double(failure?.retryAfterSeconds ?? 3600))
      }
      message = failureMessage(error)
      return false
    }
  }

  func logout() async {
    guard !isBusy else { return }
    isBusy = true
    invalidatePrivateData()
    message = nil
    defer { isBusy = false }
    do {
      _ = try await api.send(AccountRequest(path: "api/logout", method: "POST"))
      await reconcile()
      if user != nil {
        message = String(appLocalized: "Logout could not be confirmed. Please try again.")
      }
    } catch {
      await reconcile()
      if user != nil {
        message = String(appLocalized: "Logout could not be confirmed. Please try again.")
      }
    }
  }

  func updateProfile(nickname: String, pronouns: String, signature: String) async -> Bool {
    await writeUser {
      try await self.api.user(
        .json(
          "api/me", method: "PATCH",
          fields: [
            "nickname": nickname, "pronouns": pronouns, "signature": signature,
          ]))
    }
  }

  func updateAvatar(_ data: Data) async -> Bool {
    await writeUser {
      let jpeg = try AvatarEncoder.jpeg(from: data)
      let boundary = "Lycoris-\(UUID().uuidString)"
      var body = Data(
        "--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"avatar.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n"
          .utf8)
      body.append(jpeg)
      body.append(Data("\r\n--\(boundary)--\r\n".utf8))
      return try await self.api.user(
        AccountRequest(
          path: "api/me/avatar", method: "POST", body: body,
          contentType: "multipart/form-data; boundary=\(boundary)"))
    }
  }

  func changePassword(old: String, new: String) async -> Bool {
    guard !isBusy, let owner = user?.publicId else { return false }
    let token = epoch
    isBusy = true
    identityGeneration = UUID()
    message = nil
    defer { isBusy = false }
    do {
      try await verifyOwner(owner, token: token)
      _ = try await api.send(
        .json("api/me/password", fields: ["oldPassword": old, "newPassword": new]))
      invalidatePrivateData()
      let verified = await reconcile()
      if verified {
        message =
          user == nil
          ? String(appLocalized: "Password changed. Please log in again.")
          : String(appLocalized: "Password changed.")
      } else {
        message = String(
          appLocalized:
            "Password changed, but the session could not be checked. Please check your connection.")
      }
      return true
    } catch {
      guard matches(owner, token) else { return false }
      if (error as? AccountFailure)?.status == 401 { expire() }
      message =
        (error as? AccountFailure)?.status == 400
        ? String(appLocalized: "Check your current password and the new password.")
        : String(
          appLocalized:
            "The password change could not be confirmed. Check your connection, then try logging in before retrying."
        )
      await reconcile()
      return false
    }
  }

  func isBookmarked(_ id: Int64) -> Bool {
    bookmarkOverrides[id] ?? bookmarks.contains { $0.id == id }
  }

  func toggleBookmark(_ id: Int64, marker: Marker? = nil) async {
    guard id > 0, !isBusy, let owner = user?.publicId else { return }
    let token = epoch
    let saved = isBookmarked(id)
    let previousOverride = bookmarkOverrides[id]
    let previousBookmarks = bookmarks
    bookmarkTask?.cancel()
    bookmarkRevision = UUID()
    bookmarkWritePending = true
    bookmarkOverrides[id] = !saved
    if saved {
      bookmarks.removeAll { $0.id == id }
    } else if let marker, marker.id == id, !bookmarks.contains(where: { $0.id == id }) {
      bookmarks.append(marker)
    }
    isBusy = true
    identityGeneration = UUID()
    message = nil
    defer {
      isBusy = false
      bookmarkWritePending = false
    }
    do {
      try await verifyOwner(owner, token: token)
      _ = try await api.send(
        AccountRequest(path: "api/markers/\(id)/favorite", method: saved ? "DELETE" : "POST"))
      guard owner == user?.publicId, token == epoch else { return }
      // Keep the acknowledged state while refreshing only favorites. Unrelated
      // created-place and avatar requests must not hold the bookmark button busy.
      let revision = UUID()
      bookmarkRevision = revision
      bookmarkTask = Task { await refreshBookmarks(owner: owner, token: token, revision: revision) }
    } catch {
      guard matches(owner, token) else { return }
      bookmarkRevision = UUID()
      bookmarkOverrides[id] = previousOverride
      bookmarks = previousBookmarks
      handle(error, owner: owner)
    }
  }

  private func refreshBookmarks(owner: String, token: UUID, revision: UUID) async {
    do {
      let values = try await api.places("api/markers/me/favorites/details", language: language)
      guard matches(owner, token), !Task.isCancelled else { return }
      acceptBookmarks(values, revision: revision)
    } catch {
      guard matches(owner, token), revision == bookmarkRevision, !Task.isCancelled else { return }
      // A failed refresh does not undo a successful write.
      if (error as? AccountFailure)?.status == 401 { handle(error, owner: owner) }
      libraryMessage = failureMessage(error)
    }
  }

  private func acceptBookmarks(_ values: [Marker], revision: UUID) {
    guard revision == bookmarkRevision, !bookmarkWritePending else { return }
    bookmarks = values
    bookmarkOverrides.removeAll()
    hasLoadedBookmarks = true
    libraryMessage = nil
  }

  func reloadLibrary() {
    libraryTask?.cancel()
    let generation = UUID()
    libraryGeneration = generation
    libraryTask = Task { await readLibrary(generation: generation) }
  }

  func loadLibrary() async {
    libraryTask?.cancel()
    let generation = UUID()
    libraryGeneration = generation
    await readLibrary(generation: generation)
  }

  private func readLibrary(generation: UUID) async {
    guard let owner = user?.publicId, generation == libraryGeneration else { return }
    let token = epoch
    let revision = bookmarkRevision
    libraryLoading = true
    bookmarkStatusLoading = !hasLoadedBookmarks
    libraryMessage = nil
    defer {
      if generation == libraryGeneration {
        libraryLoading = false
        bookmarkStatusLoading = false
      }
    }
    do {
      let values = try await api.places("api/markers/me/favorites/details", language: language)
      guard matches(owner, token), generation == libraryGeneration else { return }
      acceptBookmarks(values, revision: revision)
      bookmarkStatusLoading = false
      let own = try await api.places("api/markers/me/created", language: language)
      guard matches(owner, token), generation == libraryGeneration else { return }
      created = own
      if user?.avatarUrl != nil {
        do {
          let bytes = try await api.send(AccountRequest(path: "api/me/avatar"))
          guard matches(owner, token), generation == libraryGeneration else { return }
          avatar = bytes
        } catch {
          guard matches(owner, token), generation == libraryGeneration else { return }
          if (error as? AccountFailure)?.status == 401 { handle(error, owner: owner) }
          // An unavailable avatar falls back to initials without hiding the library.
          avatar = nil
        }
      } else {
        avatar = nil
      }
    } catch {
      guard matches(owner, token), generation == libraryGeneration, !Task.isCancelled else {
        return
      }
      handle(error, owner: owner)
      libraryMessage = failureMessage(error)
    }
  }

  func select(_ marker: Marker) {
    closeDetail()
    guard let owner = user?.publicId else { return }
    let token = epoch
    let generation = detailGeneration
    selectedMarker = marker
    detailState = .loading
    detailTask = Task {
      do {
        let data = try await api.send(
          AccountRequest(path: "api/markers/\(marker.id)", query: ["lang": language]))
        let current = try JSONDecoder().decode(Marker.self, from: data)
        guard current.id == marker.id, current.point != nil else {
          throw PlaceFailure.invalidResponse
        }
        guard matches(owner, token), generation == detailGeneration else { return }
        selectedMarker = current
        detailState = .loaded
        if let url = PlacePresentation.imageURL(current.markImage, baseURL: api.baseURL) {
          do {
            let bytes = try await api.send(AccountRequest(path: String(url.path.dropFirst())))
            guard matches(owner, token), generation == detailGeneration else { return }
            selectedPhoto = bytes
          } catch {
            guard matches(owner, token), generation == detailGeneration else { return }
            handle(error, owner: owner)
            photoFailed = true
          }
        }
      } catch {
        guard matches(owner, token), generation == detailGeneration else { return }
        handle(error, owner: owner)
        if (error as? AccountFailure)?.status == 404 {
          libraryTask?.cancel()
          libraryGeneration = UUID()
          bookmarks.removeAll { $0.id == marker.id }
          created.removeAll { $0.id == marker.id }
          reloadLibrary()
        }
        if error is CancellationError || (error as? URLError)?.code == .cancelled { return }
        let failure: PlaceFailure
        if let known = error as? PlaceFailure {
          failure = known
        } else if error is DecodingError {
          failure = .invalidResponse
        } else if let transport = error as? URLError {
          failure = transport.code == .timedOut ? .timeout : .network
        } else if let response = error as? AccountFailure {
          failure = .http(response.status)
        } else {
          failure = .requestFailed
        }
        detailState = .failed(failure)
      }
    }
  }

  func placePhoto(_ url: URL) async throws -> Data {
    guard let owner = user?.publicId, let marker = selectedMarker,
      url == PlacePresentation.imageURL(marker.markImage, baseURL: baseURL)
        || (marker.photos ?? []).contains(where: {
          PlacePresentation.imageURL($0.url, baseURL: baseURL) == url
        })
    else { throw AccountFailure(status: 404) }
    let token = epoch
    let generation = detailGeneration
    let bytes = try await api.send(AccountRequest(path: String(url.path.dropFirst())))
    guard matches(owner, token), generation == detailGeneration else { throw CancellationError() }
    return bytes
  }

  func closeDetail() {
    detailTask?.cancel()
    detailGeneration = UUID()
    selectedMarker = nil
    selectedPhoto = nil
    photoFailed = false
    detailState = .idle
  }

  /// Shares the auth mutation gate: a logout/login cannot replace cookies between
  /// the owner check and a private contribution request. Release between chunks.
  func contributionRequest(
    _ request: AccountRequest, owner: String, token: UUID,
    waitForAccount: Bool = false,
    beforeSend: () throws -> Void = {}
  ) async throws -> Data {
    if waitForAccount {
      while isBusy || isChecking {
        guard matches(owner, token) else { throw CancellationError() }
        try await waitForAccountAccess()
        try Task.checkCancellation()
      }
      try Task.checkCancellation()
    }
    guard matches(owner, token) else { throw CancellationError() }
    guard !isBusy, !isChecking else { throw ContributionFailure.accountBusy }
    isBusy = true
    identityGeneration = UUID()
    defer { isBusy = false }
    do {
      try await verifyOwner(owner, token: token)
      try Task.checkCancellation()
      try beforeSend()
      let data = try await api.send(request)
      try Task.checkCancellation()
      guard matches(owner, token) else { throw CancellationError() }
      return data
    } catch {
      if matches(owner, token), (error as? AccountFailure)?.status == 401 { expire() }
      throw error
    }
  }

  /// A user-initiated edit read may wait for startup/foreground identity checks.
  /// Writes still use the existing fail-fast gate and are never queued for replay.
  private func waitForAccountAccess() async throws {
    let id = UUID()
    try await withTaskCancellationHandler {
      try await withCheckedThrowingContinuation {
        (continuation: CheckedContinuation<Void, any Error>) in
        if Task.isCancelled {
          continuation.resume(throwing: CancellationError())
        } else if !isBusy && !isChecking {
          continuation.resume()
        } else {
          accountWaiters[id] = continuation
        }
      }
    } onCancel: {
      Task { @MainActor [weak self] in
        self?.accountWaiters.removeValue(forKey: id)?.resume(throwing: CancellationError())
      }
    }
  }

  private func resumeAccountWaiters() {
    guard !isBusy, !isChecking else { return }
    let waiters = Array(accountWaiters.values)
    accountWaiters.removeAll()
    for waiter in waiters { waiter.resume() }
  }

  private func writeUser(_ operation: () async throws -> AccountUser) async -> Bool {
    guard !isBusy, let owner = user?.publicId else { return false }
    let token = epoch
    isBusy = true
    identityGeneration = UUID()
    message = nil
    defer { isBusy = false }
    do {
      try await verifyOwner(owner, token: token)
      let updated = try await operation()
      guard matches(owner, token) else { return false }
      guard updated.publicId == owner else { throw AccountFailure(status: 401) }
      user = updated
      avatar = nil
      reloadLibrary()
      return true
    } catch {
      guard matches(owner, token) else { return false }
      handle(error, owner: owner)
      if (error as? AccountFailure)?.status == 409 { await reconcile() }
      return false
    }
  }

  private func verifyOwner(_ owner: String, token: UUID) async throws {
    let current = try await api.user()
    guard matches(owner, token) else { throw CancellationError() }
    guard current.publicId == owner else {
      accept(current)
      reloadLibrary()
      message = AccountFailure(status: 409).message
      throw AccountFailure(status: 409)
    }
    user = current
  }

  @discardableResult private func reconcile() async -> Bool {
    do {
      accept(try await api.user())
      reloadLibrary()
      return true
    } catch {
      if (error as? AccountFailure)?.status == 401 {
        expire()
        return true
      } else {
        message = failureMessage(error)
        return false
      }
    }
  }

  private func accept(_ value: AccountUser) {
    if user?.publicId != value.publicId { invalidatePrivateData() }
    user = value
    hasVerifiedIdentity = true
  }

  private func matches(_ owner: String, _ token: UUID) -> Bool {
    user?.publicId == owner && epoch == token
  }
  private func expire() {
    invalidatePrivateData()
    user = nil
    hasVerifiedIdentity = true
  }

  private func invalidatePrivateData() {
    onPrivateDataInvalidated?()
    epoch = UUID()
    identityGeneration = UUID()
    libraryTask?.cancel()
    bookmarkTask?.cancel()
    bookmarkRevision = UUID()
    bookmarkOverrides.removeAll()
    hasLoadedBookmarks = false
    bookmarkStatusLoading = false
    libraryGeneration = UUID()
    libraryLoading = false
    libraryMessage = nil
    bookmarks = []
    created = []
    avatar = nil
    closeDetail()
  }

  private func handle(_ error: Error, owner: String) {
    guard user?.publicId == owner else { return }
    if (error as? AccountFailure)?.status == 401 { expire() }
    message = failureMessage(error)
  }

  private func failureMessage(_ error: Error) -> String {
    if error is SessionStorageFailure {
      return String(appLocalized: "Could not save your session securely. Please try again.")
    }
    if let error = error as? URLError {
      switch error.code {
      case .notConnectedToInternet, .dataNotAllowed:
        return String(
          appLocalized:
            "No internet connection. Check your connection and allow Lycoris to use Wi-Fi or cellular data in Settings.",
          table: "Network")
      case .timedOut:
        return String(appLocalized: "The request timed out. Please try again.", table: "Network")
      default: break
      }
    }
    return (error as? AccountFailure)?.message ?? AccountFailure(status: 0).message
  }
}
