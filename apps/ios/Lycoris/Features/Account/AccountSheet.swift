import PhotosUI
import SwiftUI

enum AccountDestination: Hashable { case profile, password, bookmarks, created }

struct AccountSheet: View {
  @Bindable var store: AccountStore
  var destination: AccountDestination = .profile
  var contributions: ContributionStore? = nil
  var onDraft: (UUID) -> Void = { _ in }
  var onAuthenticated: () -> Void = {}
  let onSelect: (Marker) -> Void
  @Environment(\.dismiss) private var dismiss
  @State private var path: [AccountDestination] = []

  var body: some View {
    Group {
      if let user = store.user {
        NavigationStack(path: $path) {
          content(destination, user: user)
            .navigationDestination(for: AccountDestination.self) { content($0, user: user) }
        }
        .id(user.publicId)
      } else {
        AuthenticationView(store: store, onAuthenticated: onAuthenticated) { dismiss() }
      }
    }
    .presentationDragIndicator(.visible)
    .interactiveDismissDisabled(store.isBusy)
    .onChange(of: store.user?.publicId) { _, _ in path = [] }
  }

  @ViewBuilder private func content(_ destination: AccountDestination, user: AccountUser)
    -> some View
  {
    Group {
      switch destination {
      case .profile: ProfileView(store: store, user: user)
      case .password: PasswordView(store: store)
      case .bookmarks, .created:
        AccountPlacesView(
          store: store, created: destination == .created, contributions: contributions,
          onDraft: onDraft
        ) { marker in
          dismiss()
          onSelect(marker)
        }
      }
    }.toolbar {
      ToolbarItem(placement: .topBarTrailing) {
        Button("Done") { dismiss() }.accessibilityIdentifier("account.close")
      }
    }
  }
}

struct AccountAvatar: View {
  var user: AccountUser?
  var data: Data?
  var size: CGFloat = 38
  var body: some View {
    ZStack {
      Circle().fill(
        LinearGradient(
          colors: [Color("AvatarTop"), Color("AvatarBottom")], startPoint: .top, endPoint: .bottom))
      if let data, let image = UIImage(data: data) {
        Image(uiImage: image).resizable().scaledToFill()
      } else if user == nil {
        Image(systemName: "person.fill").font(.system(size: size * 0.48)).foregroundStyle(.white)
      } else {
        Text(user?.initials ?? "").font(.system(size: size * 18 / 38, weight: .bold))
          .foregroundStyle(.white).lineLimit(1).minimumScaleFactor(0.5)
          .padding(3)
      }
    }.frame(width: size, height: size).clipShape(Circle()).accessibilityHidden(true)
  }
}

private struct ProfileView: View {
  @Bindable var store: AccountStore
  let user: AccountUser
  @State private var nickname = ""
  @State private var pronouns = ""
  @State private var signature = ""
  @State private var photo: PhotosPickerItem?
  @State private var readingPhoto = false
  @State private var saved = false
  @FocusState private var field: ProfileField?
  private enum ProfileField { case nickname, pronouns, signature }

  var body: some View {
    let avatarData = store.avatar
    let isReadingPhoto = readingPhoto
    Form {
      Section {
        PhotosPicker(selection: $photo, matching: .images, photoLibrary: .shared()) {
          HStack {
            AccountAvatar(user: user, data: avatarData, size: 64)
            Text("Change avatar")
            if isReadingPhoto { ProgressView() }
          }
        }.disabled(store.isBusy || readingPhoto).accessibilityIdentifier("profile.avatar")
        LabeledContent("Username", value: user.username ?? "—")
        LabeledContent("Email", value: user.email ?? "—")
      }
      Section {
        TextField("Nickname", text: $nickname).accessibilityIdentifier("profile.nickname")
          .focused($field, equals: .nickname).submitLabel(.next).onSubmit { field = .pronouns }
        TextField("Pronouns", text: $pronouns).accessibilityIdentifier("profile.pronouns")
          .focused($field, equals: .pronouns).submitLabel(.next).onSubmit { field = .signature }
        TextField("Signature", text: $signature, axis: .vertical).lineLimit(2...5)
          .accessibilityIdentifier("profile.signature").focused($field, equals: .signature)
        Button("Save") {
          field = nil
          Task {
            saved = await store.updateProfile(
              nickname: nickname, pronouns: pronouns, signature: signature)
          }
        }
        .disabled(
          store.isBusy
            || !AccountValidation.profile(
              nickname: nickname, pronouns: pronouns, signature: signature)
        )
        .accessibilityIdentifier("profile.save")
        if saved { Text("Profile saved.").foregroundStyle(.secondary) }
        if !AccountValidation.profile(nickname: nickname, pronouns: pronouns, signature: signature)
        {
          Text("Maximum lengths: nickname 255, pronouns 64, signature 200.").font(.footnote)
        }
      }.disabled(store.isBusy)
      Section {
        NavigationLink("Change password", value: AccountDestination.password)
        NavigationLink("My Places", value: AccountDestination.created)
        NavigationLink("Bookmarks", value: AccountDestination.bookmarks)
        Button("Log out", role: .destructive) { Task { await store.logout() } }
          .accessibilityIdentifier("profile.logout")
      }.disabled(store.isBusy)
      if let message = store.message {
        Section { Text(message).accessibilityIdentifier("account.message") }
      }
    }
    .navigationTitle("Account").navigationBarTitleDisplayMode(.inline)
    .scrollDismissesKeyboard(.interactively)
    .toolbar {
      ToolbarItemGroup(placement: .keyboard) {
        Spacer()
        Button("Done") { field = nil }
      }
    }
    .refreshable {
      await store.restore()
      if let current = store.user {
        nickname = current.nickname ?? ""
        pronouns = current.pronouns ?? ""
        signature = current.signature ?? ""
        saved = false
      }
    }
    .task {
      nickname = user.nickname ?? ""
      pronouns = user.pronouns ?? ""
      signature = user.signature ?? ""
    }
    .onChange(of: photo) { _, selected in
      guard let selected else { return }
      let owner = user.publicId
      let token = store.epoch
      readingPhoto = true
      Task {
        defer {
          readingPhoto = false
          photo = nil
        }
        do {
          guard let data = try await selected.loadTransferable(type: Data.self),
            store.user?.publicId == owner, store.epoch == token
          else { return }
          _ = await store.updateAvatar(data)
        } catch {
          store.message = String(appLocalized: "Could not read that photo. Please choose another.")
        }
      }
    }
    .onChange(of: nickname) { _, _ in saved = false }
    .onChange(of: pronouns) { _, _ in saved = false }
    .onChange(of: signature) { _, _ in saved = false }
  }
}

private struct PasswordView: View {
  @Bindable var store: AccountStore
  @State private var old = ""
  @State private var new = ""
  @State private var confirmation = ""
  @FocusState private var field: PasswordField?
  private enum PasswordField { case old, new, confirmation }
  var body: some View {
    Form {
      Section {
        SecureField("Current password", text: $old).textContentType(.password)
          .accessibilityIdentifier("password.old")
          .focused($field, equals: .old).submitLabel(.next).onSubmit { field = .new }
        SecureField("New password", text: $new).textContentType(.newPassword)
          .accessibilityIdentifier("password.new")
          .focused($field, equals: .new).submitLabel(.next).onSubmit { field = .confirmation }
        SecureField("Confirm password", text: $confirmation).textContentType(.newPassword)
          .accessibilityIdentifier("password.confirm")
          .focused($field, equals: .confirmation).submitLabel(.done).onSubmit { field = nil }
      } footer: {
        Text("Use at least 4 characters and no more than 72 UTF-8 bytes.")
      }
      Section {
        if !confirmation.isEmpty && new != confirmation {
          Text("The new passwords do not match.")
        }
        Button("Change password") {
          field = nil
          Task {
            _ = await store.changePassword(old: old, new: new)
            old = ""
            new = ""
            confirmation = ""
          }
        }
        .disabled(
          store.isBusy || old.isEmpty || !AccountValidation.password(new) || new != confirmation
        )
        .accessibilityIdentifier("password.submit")
        if let message = store.message { Text(message).accessibilityIdentifier("account.message") }
      }
    }
    .autocorrectionDisabled().textInputAutocapitalization(.never)
    .navigationTitle("Change password").navigationBarTitleDisplayMode(.inline)
    .disabled(store.isBusy)
    .scrollDismissesKeyboard(.interactively)
    .onDisappear {
      old = ""
      new = ""
      confirmation = ""
    }
  }
}

struct AccountPlacesView: View {
  @Bindable var store: AccountStore
  let created: Bool
  var contributions: ContributionStore? = nil
  var onDraft: (UUID) -> Void = { _ in }
  let onSelect: (Marker) -> Void
  private var places: [Marker] { created ? store.created : store.bookmarks }
  var body: some View {
    List {
      if created, let contributions {
        Section("Drafts") {
          ForEach(contributions.drafts) { draft in
            Button {
              onDraft(draft.id)
            } label: {
              VStack(alignment: .leading) {
                Text(
                  draft.fields.title.isEmpty
                    ? String(appLocalized: "Untitled place") : draft.fields.title)
                if let date = contributions.modifiedAt(draft.id) {
                  Text(date, format: .dateTime.year().month().day().hour().minute())
                    .font(.caption).foregroundStyle(.secondary)
                }
                Text("Continue").font(.caption).foregroundStyle(.secondary)
              }
            }.disabled(contributions.isWorking)
              .swipeActions {
                Button("Delete draft", role: .destructive) {
                  do { try contributions.deleteDraft(draft.id) } catch {
                    store.message = String(
                      appLocalized: "Could not save the contribution on this device.")
                  }
                }.disabled(contributions.isWorking)
              }
          }
          Text("Drafts are saved on this device only.").font(.caption).foregroundStyle(.secondary)
          if let warning = contributions.draftRecoveryMessage {
            Text(warning).foregroundStyle(.secondary)
              .accessibilityIdentifier("contribution.recovery-warning")
          }
          if let message = contributions.message { Text(message) }
        }
      }
      if store.libraryLoading { ProgressView() }
      if let message = store.message, message != store.libraryMessage {
        Text(message).accessibilityIdentifier("account.message")
      }
      if let message = store.libraryMessage {
        Text(message)
        Button("Retry") { store.reloadLibrary() }
      } else if !store.libraryLoading && places.isEmpty {
        Text(created ? "No places yet." : "No bookmarks yet.").foregroundStyle(.secondary)
      }
      ForEach(places, id: \.id) { marker in
        VStack(alignment: .leading, spacing: 0) {
          PlaceRow(
            place: PlacePresentation(
              marker: marker, origin: nil, located: false, baseURL: store.baseURL)
          ) {
            onSelect(marker)
          }
          if created {
            HStack {
              Text(marker.isPublic == true ? "Public" : "Private")
              Text(reviewLabel(marker.reviewStatus))
            }.font(.caption).foregroundStyle(.secondary).padding(.horizontal, 16).padding(
              .bottom, 10)
          }
        }.listRowInsets(EdgeInsets())
          .swipeActions {
            if !created {
              Button("Remove bookmark", role: .destructive) {
                Task { await store.toggleBookmark(marker.id) }
              }
              .disabled(store.isBusy)
            }
          }
      }
    }
    .navigationTitle(created ? "My Places" : "Bookmarks").navigationBarTitleDisplayMode(.inline)
    .onAppear {
      store.message = nil
      contributions?.refreshDrafts()
    }
    .refreshable { await store.loadLibrary() }
  }

  private func reviewLabel(_ value: String?) -> String {
    switch value {
    case "APPROVED": String(appLocalized: "Approved")
    case "PENDING": String(appLocalized: "Pending review")
    case "REJECTED": String(appLocalized: "Rejected")
    default: String(appLocalized: "Unknown review status")
    }
  }
}
