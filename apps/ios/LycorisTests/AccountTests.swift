import Foundation
import ImageIO
import Testing

@testable import Lycoris

@MainActor
struct AccountTests {
  @Test func networkDenialExplainsRecoveryAndDoesNotInventALogin() async {
    let store = AccountStore(api: DisconnectedAccountAPI())
    #expect(
      !(await store.authenticate(
        username: "fixture", email: "", password: "fixture", register: false)))
    #expect(store.user == nil && !store.isBusy)
    #expect(
      store.message
        == String(
          appLocalized:
            "No internet connection. Check your connection and allow Lycoris to use Wi-Fi or cellular data in Settings.",
          table: "Network"))
  }

  @Test func emailVerificationNormalizesAddressAndUsesPurposeBoundCode() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    try await store.sendEmailCode(email: " User@Example.test ", reset: false)
    #expect(await api.emailFields?["email"] == "user@example.test")
    #expect(await api.emailFields?["purpose"] == "register")
    #expect(
      await store.authenticate(
        username: "fixture", email: " User@Example.test ", password: "new-password", register: true,
        verificationCode: "123456"))
    #expect(await api.loginFields?["verificationCode"] == "123456")
    #expect(await api.loginFields?["email"] == "user@example.test")
  }

  @Test func recoveryInvalidatesPrivateDataAndReturnsToAnonymous() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    #expect(!store.bookmarks.isEmpty)
    await api.setFailure(503)  // Recovery must not depend on a subsequent session read.
    #expect(
      await store.resetPassword(
        email: " User@Example.test ", code: "123456", password: "new-password"))
    #expect(await api.emailFields?["email"] == "user@example.test")
    #expect(await api.emailFields?["verificationCode"] == "123456")
    #expect(await api.emailFields?["newPassword"] == "new-password")
    #expect(store.user == nil && store.bookmarks.isEmpty && store.created.isEmpty)
    #expect(
      store.message == String(appLocalized: "Password reset. Please log in with your new password.")
    )
  }

  @Test func wrongCodeLockUsesServerDeadlineWithoutPretendingRecoverySucceeded() async {
    let api = AccountFixture()
    await api.lockVerification()
    let store = AccountStore(api: api)
    let before = Date()
    #expect(
      !(await store.resetPassword(
        email: "user@example.test", code: "000000", password: "new-password")))
    #expect(store.verificationLockedUntil.timeIntervalSince(before) >= 3590)
    #expect(
      store.message == String(appLocalized: "Too many incorrect codes. Try again in one hour."))
  }

  private func waitFor(_ condition: () async -> Bool) async throws {
    for _ in 0..<200 {
      if await condition() { return }
      try await Task.sleep(for: .milliseconds(5))
    }
    Issue.record("Account state did not settle")
  }

  @Test func lateProfileSuccessCannotRestoreExpiredIdentity() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    await api.hold("PATCH api/me")
    let saving = Task { await store.updateProfile(nickname: "New", pronouns: "", signature: "") }
    try await waitFor { await api.hasPending }
    await api.expireLibrary()
    await store.loadLibrary()
    #expect(store.user == nil)
    await api.release()
    #expect(await saving.value == false)
    #expect(store.user == nil && store.bookmarks.isEmpty)
  }

  @Test func oldRestoreCannotOverwriteSavedProfile() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await api.hold("GET api/me")
    let restoring = Task { await store.restore() }
    try await waitFor { await api.hasPending }
    #expect(await store.updateProfile(nickname: "New", pronouns: "", signature: ""))
    #expect(store.user?.nickname == "New")
    await api.release()
    await restoring.value
    #expect(store.user?.nickname == "New")
  }

  @Test func latePrivatePhotoCannotReturnAfterLogout() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    let marker = try #require(store.bookmarks.first)
    await api.hold("GET uploads/markers/fixture.jpg")
    store.select(marker)
    try await waitFor { await api.hasPending }
    #expect(store.selectedMarker != nil)
    await store.logout()
    await api.release()
    await Task.yield()
    #expect(store.selectedMarker == nil && store.selectedPhoto == nil && store.avatar == nil)
  }

  @Test func authTransitionsCannotOverlap() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await api.hold("POST api/login")
    let login = Task {
      await store.authenticate(username: "account-b", email: "", password: "test", register: false)
    }
    try await waitFor { await api.hasPending }
    await store.logout()
    #expect(await api.logoutWrites == 0)
    await api.release()
    #expect(await login.value)
    #expect(store.user?.publicId == "account-b")
  }

  @Test func outageKeepsIdentityButUnauthorizedClearsEveryPrivateValue() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    #expect(store.user?.publicId == "account-a")
    #expect(store.bookmarks.count == 1)
    await api.setFailure(503)
    await store.restore()
    #expect(store.user?.publicId == "account-a")
    #expect(store.message != nil)
    await api.setFailure(401)
    await store.restore()
    #expect(store.user == nil && store.bookmarks.isEmpty && store.created.isEmpty)
    #expect(store.avatar == nil && store.selectedMarker == nil)
  }

  @Test func delayedOldBookmarksCannotReturnAfterLogoutAndLogin() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    await api.holdFavorites()
    let read = Task { await store.loadLibrary() }
    await api.waitUntilHeld()
    await store.logout()
    #expect(store.user == nil && store.bookmarks.isEmpty)
    let success = await store.authenticate(
      username: "account-b", email: "", password: "test", register: false)
    #expect(success && store.user?.publicId == "account-b")
    await store.loadLibrary()
    await api.releaseFavorites()
    await read.value
    #expect(store.bookmarks.isEmpty)
  }

  @Test func failedLogoutDoesNotClaimAnonymous() async {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await api.failLogout()
    await store.logout()
    #expect(store.user?.publicId == "account-a")
    #expect(
      store.message == String(appLocalized: "Logout could not be confirmed. Please try again."))
  }

  @Test func identityRecheckPreventsWriteAsAnotherAccount() async {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await api.switchUser("account-b")
    let saved = await store.updateProfile(nickname: "Do not write", pronouns: "", signature: "")
    #expect(!saved && store.user?.publicId == "account-b")
    #expect(await api.profileWrites == 0)
  }

  @Test func emptyFavoriteResponseRefreshesListAndPasswordClearCookieExpiresSession() async {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    #expect(store.isBookmarked(1))
    await store.toggleBookmark(1)
    #expect(!store.isBookmarked(1))
    #expect(await store.changePassword(old: "old", new: "new1"))
    #expect(store.user == nil && store.bookmarks.isEmpty)
    #expect(store.message == String(appLocalized: "Password changed. Please log in again."))
  }

  @Test func loginTrimsIdentityWithoutChangingUsernameOrPasswordCase() async {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    #expect(
      await store.authenticate(
        username: "  User@Handle  ", email: "", password: "  Case Sensitive  ", register: false))
    #expect(await api.loginFields == ["username": "User@Handle", "password": "  Case Sensitive  "])
  }

  @Test func registrationRefusesEmailShapedUsernamesWithoutLeavingTheCurrentSession() async {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    #expect(
      !(await store.authenticate(
        username: "victim@example.test", email: "new@example.test", password: "new-password",
        register: true, verificationCode: "123456")))
    #expect(await api.loginFields == nil)
    #expect(store.user?.publicId == "account-a")
    #expect(store.message == String(appLocalized: "Usernames can't contain @."))
    #expect(AccountValidation.newUsername("plain-name"))
    #expect(!AccountValidation.newUsername("name@"))
  }

  @Test func serverUsernameRuleIsNotReportedAsADuplicate() async {
    let api = AccountFixture()
    await api.reject("api/register", with: AccountFailure(status: 400, code: 4005))
    let store = AccountStore(api: api)
    #expect(
      !(await store.authenticate(
        username: "fixture", email: "new@example.test", password: "new-password", register: true,
        verificationCode: "123456")))
    #expect(store.message == String(appLocalized: "Usernames can't contain @."))
  }

  @Test func rateLimitedPasswordChangeSaysNothingChangedAndKeepsTheSession() async {
    let api = AccountFixture()
    await api.reject(
      "api/me/password", with: AccountFailure(status: 429, code: 42934, retryAfterSeconds: 900))
    let store = AccountStore(api: api)
    await store.restore()
    #expect(!(await store.changePassword(old: "guess", new: "new-password")))
    #expect(store.user?.publicId == "account-a")
    #expect(store.message == String(appLocalized: "Too many attempts. Please try again later."))
  }

  @Test func bookmarkRespondsBeforePreflightAndDoesNotWaitForLibraryRefresh() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    let marker = try #require(store.bookmarks.first)
    await api.hold("GET api/me")
    let saving = Task { await store.toggleBookmark(1) }
    try await waitFor { await api.hasPending }
    #expect(!store.isBookmarked(1) && store.bookmarks.isEmpty)
    await api.holdFavorites()
    let createdReads = await api.createdReads
    await api.release()
    await saving.value
    await api.waitUntilHeld()
    #expect(!store.isBusy && !store.isBookmarked(1))
    #expect(await api.createdReads == createdReads)
    await api.releaseFavorites(empty: true)
    await store.toggleBookmark(1, marker: marker)
    #expect(store.isBookmarked(1) && store.bookmarks.first?.id == 1)
  }

  @Test func failedBookmarkWriteRollsBackAndAccountChangeNeverWritesAsSomeoneElse() async {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    await api.rejectFavorite(503)
    await store.toggleBookmark(1)
    #expect(store.isBookmarked(1) && store.bookmarks.first?.id == 1 && store.message != nil)
    await api.switchUser("account-b")
    await store.toggleBookmark(1)
    #expect(store.user?.publicId == "account-b" && !store.isBookmarked(1))
    #expect(await api.favoriteWrites == 1)
  }

  @Test func bookmarkStatusIsReadyBeforeCreatedPlacesFinishLoading() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await api.hold("GET api/markers/me/created")
    await store.restore()
    try await waitFor { await api.hasPending }
    #expect(store.isBookmarked(1))
    #expect(!store.bookmarkStatusLoading && store.libraryLoading)
    await api.release()
  }

  @Test func staleLibraryCannotUndoAnAcknowledgedBookmarkChange() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    await api.holdFavorites()
    let oldRead = Task { await store.loadLibrary() }
    await api.waitUntilHeld()
    await store.toggleBookmark(1)
    await api.releaseFavorites()
    await oldRead.value
    #expect(!store.isBookmarked(1) && store.bookmarks.isEmpty)
  }

  @Test func failedRefreshKeepsSuccessfulBookmarkAndExpiryClearsIt() async throws {
    let api = AccountFixture()
    let store = AccountStore(api: api)
    await store.restore()
    await store.loadLibrary()
    await api.rejectFavoritesRead(503)
    await store.toggleBookmark(1)
    try await waitFor { store.libraryMessage != nil }
    #expect(!store.isBookmarked(1) && store.bookmarks.isEmpty && !store.isBusy)
    await api.setFailure(401)
    await store.restore()
    #expect(store.user == nil && !store.isBookmarked(1))
  }

  @Test func passwordAndProfileLimitsMatchRustUnits() {
    #expect(AccountValidation.password("😀😀"))
    #expect(!AccountValidation.password("😀"))
    #expect(AccountValidation.password(String(repeating: "a", count: 72)))
    #expect(!AccountValidation.password(String(repeating: "é", count: 37)))
    #expect(
      AccountValidation.profile(
        nickname: "", pronouns: "", signature: String(repeating: "a", count: 200)))
    #expect(
      !AccountValidation.profile(
        nickname: "", pronouns: "", signature: String(repeating: "a", count: 201)))
  }

  @Test func avatarConvertsToSmallJPEGAndRejectsInvalidInput() throws {
    let png = Data(
      base64Encoded:
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aY9sAAAAASUVORK5CYII="
    )!
    let jpeg = try AvatarEncoder.jpeg(from: png)
    #expect(jpeg.starts(with: [0xff, 0xd8]))
    let source = try #require(CGImageSourceCreateWithData(jpeg as CFData, nil))
    let image = try #require(CGImageSourceCreateImageAtIndex(source, 0, nil))
    #expect(image.width == 1 && image.height == 1)
    #expect(throws: AccountFailure(status: 400)) {
      try AvatarEncoder.jpeg(from: Data("invalid".utf8))
    }
  }
}

private struct DisconnectedAccountAPI: AccountServing {
  let baseURL = URL(string: "https://offline.example.test")
  func send(_ request: AccountRequest) async throws -> Data {
    throw URLError(.notConnectedToInternet)
  }
}

private actor AccountFixture: AccountServing {
  nonisolated let baseURL = URL(string: "https://accounts.example.test")
  var identity: String? = "account-a"
  var failure: Int?
  var logoutFails = false
  var favorite = true
  var favoriteFailure: Int?
  var favoritesReadFailure: Int?
  var favoriteWrites = 0
  var createdReads = 0
  var loginFields: [String: String]?
  var emailFields: [String: String]?
  var verificationLocked = false
  var pathFailures: [String: AccountFailure] = [:]
  func lockVerification() { verificationLocked = true }
  func reject(_ path: String, with failure: AccountFailure) { pathFailures[path] = failure }
  func rejectFavorite(_ status: Int) { favoriteFailure = status }
  func rejectFavoritesRead(_ status: Int) { favoritesReadFailure = status }
  var shouldHold = false
  var held: CheckedContinuation<Data, Never>?
  var ready: CheckedContinuation<Void, Never>?
  var holdRequest: String?
  var pending: CheckedContinuation<Data, Never>?
  var pendingData = Data()
  var libraryExpired = false
  var nickname: String?
  private(set) var logoutWrites = 0
  var hasPending: Bool { pending != nil }
  func hold(_ request: String) { holdRequest = request }
  func release() {
    pending?.resume(returning: pendingData)
    pending = nil
  }
  func expireLibrary() { libraryExpired = true }
  private(set) var profileWrites = 0
  func setFailure(_ value: Int) { failure = value }
  func failLogout() { logoutFails = true }
  func switchUser(_ id: String) { identity = id }
  func holdFavorites() { shouldHold = true }
  func waitUntilHeld() async {
    if held != nil { return }
    await withCheckedContinuation { ready = $0 }
  }
  func releaseFavorites(empty: Bool = false) {
    held?.resume(returning: Data((empty ? "[]" : Self.markers).utf8))
    held = nil
  }
  static let markers =
    #"[{"id":1,"version":1,"lat":31,"lng":121,"category":"accessible_toilet","title":"Owned place","contentLanguage":"en","isPublic":false,"reviewStatus":"PENDING"}]"#

  func send(_ request: AccountRequest) async throws -> Data {
    let data = try await response(request)
    if holdRequest == "\(request.method) \(request.path)" {
      holdRequest = nil
      pendingData = data
      return await withCheckedContinuation { pending = $0 }
    }
    return data
  }

  private func response(_ request: AccountRequest) async throws -> Data {
    if let failure = pathFailures[request.path] { throw failure }
    switch request.path {
    case "api/me":
      if let failure { throw AccountFailure(status: failure) }
      guard let identity else { throw AccountFailure(status: 401) }
      if request.method == "PATCH" {
        profileWrites += 1
        nickname = try JSONDecoder().decode([String: String].self, from: request.body!)["nickname"]
      }
      return Data(
        "{\"code\":0,\"data\":{\"publicId\":\"\(identity)\",\"username\":\"\(identity)\",\"nickname\":\"\(nickname ?? identity)\"}}"
          .utf8)
    case "api/auth/email-code", "api/auth/reset-password":
      if verificationLocked {
        throw AccountFailure(status: 429, code: 42931, retryAfterSeconds: 3598)
      }
      emailFields = try JSONDecoder().decode([String: String].self, from: request.body!)
      if request.path == "api/auth/reset-password" { identity = nil }
      return Data(#"{"code":0,"data":null}"#.utf8)
    case "api/login", "api/register":
      let fields = try JSONDecoder().decode([String: String].self, from: request.body!)
      loginFields = fields
      identity = fields["username"]
      return try await send(AccountRequest(path: "api/me"))
    case "api/logout":
      logoutWrites += 1
      if logoutFails { throw AccountFailure(status: 503) }
      identity = nil
      return Data(#"{"code":0,"data":null}"#.utf8)
    case "api/me/password":
      identity = nil
      return Data(#"{"code":0,"data":null}"#.utf8)
    case "api/markers/me/favorites/details":
      if let favoritesReadFailure { throw AccountFailure(status: favoritesReadFailure) }
      if libraryExpired { throw AccountFailure(status: 401) }
      if shouldHold {
        shouldHold = false
        return await withCheckedContinuation {
          held = $0
          ready?.resume()
          ready = nil
        }
      }
      return Data((identity == "account-a" && favorite ? Self.markers : "[]").utf8)
    case "api/markers/me/created":
      createdReads += 1
      return Data("[]".utf8)
    case "api/markers/1":
      return Data(
        #"{"id":1,"version":1,"lat":31,"lng":121,"category":"accessible_toilet","title":"Owned place","contentLanguage":"en","isPublic":false,"reviewStatus":"PENDING","markImage":"/uploads/markers/fixture.jpg"}"#
          .utf8)
    case "uploads/markers/fixture.jpg": return Data([1, 2, 3])
    case "api/markers/1/favorite":
      favoriteWrites += 1
      if let favoriteFailure { throw AccountFailure(status: favoriteFailure) }
      favorite = request.method == "POST"
      return Data()
    default: throw AccountFailure(status: 404)
    }
  }
}
