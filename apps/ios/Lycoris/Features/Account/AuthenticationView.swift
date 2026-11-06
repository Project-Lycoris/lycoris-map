import SwiftUI

struct AuthenticationView: View {
  @Bindable var store: AccountStore
  let onAuthenticated: () -> Void
  let onClose: () -> Void
  @State private var registering = false
  @State private var resetting = false

  var body: some View {
    NavigationStack {
      credentials(registering: false)
        .navigationDestination(isPresented: $resetting) {
          CredentialForm(
            store: store, registering: false, resetting: true,
            onSwitch: { resetting = false }, onForgot: {},
            onAuthenticated: onAuthenticated, onClose: onClose)
        }
        .navigationDestination(isPresented: $registering) {
          credentials(registering: true)
        }
    }
    .onChange(of: registering) { _, _ in store.message = nil }
  }

  private func credentials(registering: Bool) -> some View {
    CredentialForm(
      store: store, registering: registering, resetting: false,
      onSwitch: { self.registering.toggle() }, onForgot: { resetting = true },
      onAuthenticated: onAuthenticated, onClose: onClose)
  }
}

private struct CredentialForm: View {
  @Bindable var store: AccountStore
  let registering: Bool
  let resetting: Bool
  let onSwitch: () -> Void
  let onForgot: () -> Void
  let onAuthenticated: () -> Void
  let onClose: () -> Void
  @State private var username = ""
  @State private var email = ""
  @State private var password = ""
  @State private var confirmation = ""
  @State private var code = ""
  @State private var resendAt = Date.distantPast
  @State private var localLock = Date.distantPast
  @State private var now = Date()
  private var verification: Bool { registering || resetting }
  private var serverLock: Date {
    email.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
      == store.verificationLockedEmail
      ? store.verificationLockedUntil : .distantPast
  }
  private var locked: Bool { verification && max(localLock, serverLock) > now }
  private var seconds: Int {
    max(
      0,
      Int(ceil(max(resendAt, max(localLock, serverLock)).timeIntervalSince(now)))
    )
  }
  @FocusState private var focus: Field?
  private enum Field { case email, username, password, code }
  private var unavailable: Bool { store.isBusy || store.isChecking }
  private var usernameRejected: Bool {
    registering && !AccountValidation.newUsername(username)
  }

  var body: some View {
    Form {
      Section {
        if verification {
          TextField("Email", text: $email)
            .keyboardType(.emailAddress).textContentType(.emailAddress)
            .focused($focus, equals: .email).submitLabel(.next)
            .onSubmit { focus = resetting ? .password : .username }
            .accessibilityIdentifier("auth.email")
        }
        if !resetting {
          TextField(registering ? "Username" : "Email or Username", text: $username)
            .textContentType(.username)
            .focused($focus, equals: .username).submitLabel(.next)
            .onSubmit { focus = .password }
            .accessibilityIdentifier("auth.username")
        }
        SecureField(resetting ? "New Password" : "Password", text: $password)
          .textContentType(verification ? .newPassword : .password)
          .focused($focus, equals: .password).submitLabel(.go)
          .onSubmit(submit)
          .accessibilityIdentifier("auth.password")
        if resetting {
          SecureField("Confirm Password", text: $confirmation).textContentType(.newPassword)
        }
      } footer: {
        if verification {
          VStack(alignment: .leading, spacing: 4) {
            if usernameRejected {
              Text("Usernames can't contain @.")
                .accessibilityIdentifier("auth.usernameRule")
            }
            Text("Use at least 4 characters and no more than 72 UTF-8 bytes.")
          }
        }
      }
      .disabled(unavailable)
      .onChange(of: usernameRejected) { _, rejected in
        // The footer is not read while typing; say why Register stopped being available.
        if rejected {
          AccessibilityNotification.Announcement(
            String(appLocalized: "Usernames can't contain @.")
          ).post()
        }
      }

      if verification {
        Section {
          TextField("Verification Code", text: $code)
            .keyboardType(.numberPad).textContentType(.oneTimeCode)
            .focused($focus, equals: .code)
            .onChange(of: code) { _, value in
              code = String(value.filter { $0.isASCII && $0.isNumber }.prefix(6))
            }
            .disabled(unavailable || locked)
            .accessibilityIdentifier("auth.verification")
          Button(action: sendCode) {
            if seconds > 0 { Text("Resend in \(seconds)s") } else { Text("Send code") }
          }
          .disabled(unavailable || seconds > 0 || !email.contains("@"))
          .accessibilityIdentifier("auth.sendCode")
        }
      }

      Section {
        Button(action: submit) {
          HStack {
            Spacer(minLength: 0)
            if unavailable { ProgressView() }
            Text(resetting ? "Reset Password" : registering ? "Register" : "Login")
            Spacer(minLength: 0)
          }
        }
        .disabled(!canSubmit || unavailable)
        .accessibilityIdentifier("auth.submit")
        if let message = store.message {
          Text(message).foregroundStyle(.secondary)
            .accessibilityIdentifier("account.message")
        }
      }

      Section {
        Button(action: switchMode) {
          Text(
            resetting
              ? "Back to login"
              : registering ? "Already have an account? Login" : "No account? Register")
        }
        .disabled(unavailable)
        .accessibilityIdentifier("auth.switch")
        if !verification {
          Button("Forgot password?", action: onForgot).disabled(unavailable)
        }
      } footer: {
        if !resetting {
          Text("Apple and Google login are not enabled yet. Please use your account password.")
        }
      }
    }
    .autocorrectionDisabled().textInputAutocapitalization(.never)
    .scrollDismissesKeyboard(.interactively)
    .navigationTitle(resetting ? "Reset Password" : registering ? "Register" : "Login")
    .onChange(of: email) { old, new in
      code = ""
      if old.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        != new.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
      {
        resendAt = .distantPast
        localLock = .distantPast
        store.message = nil
      }
    }
    .task {
      while !Task.isCancelled {
        now = Date()
        do { try await Task.sleep(for: .seconds(1)) } catch { return }
      }
    }
    .navigationBarTitleDisplayMode(.inline)
    .navigationBarBackButtonHidden(store.isBusy)
    .toolbar {
      ToolbarItem(placement: .topBarTrailing) {
        Button("Close account", systemImage: "xmark", action: onClose)
          .labelStyle(.iconOnly)
          .disabled(store.isBusy)
          .accessibilityIdentifier("account.close")
      }
      ToolbarItemGroup(placement: .keyboard) {
        Spacer()
        Button("Done") { focus = nil }
      }
    }
    .onDisappear {
      password = ""
      focus = nil
    }
  }

  private var canSubmit: Bool {
    let identity = username.trimmingCharacters(in: .whitespacesAndNewlines)
    guard !identity.isEmpty || resetting, !password.isEmpty, !locked, !usernameRejected else {
      return false
    }
    if !verification { return true }
    guard code.count == 6, !resetting || password == confirmation else { return false }
    return identity.unicodeScalars.count <= 255 && email.contains("@")
      && email.unicodeScalars.count <= 255 && AccountValidation.password(password)
  }

  private func switchMode() {
    password = ""
    focus = nil
    onSwitch()
  }

  private func sendCode() {
    guard !unavailable, seconds == 0 else { return }
    Task {
      do {
        try await store.sendEmailCode(email: email, reset: resetting)
        resendAt = Date().addingTimeInterval(60)
        code = ""
        store.message = String(appLocalized: "A code has been sent. It expires in 10 minutes.")
      } catch {
        let failure = error as? AccountFailure
        store.message = failure?.message ?? AccountFailure(status: 0).message
        if failure?.status == 429 {
          resendAt = Date().addingTimeInterval(Double(failure?.retryAfterSeconds ?? 60))
          if failure?.code == 42931 { localLock = resendAt }
        }
      }
    }
  }

  private func submit() {
    guard canSubmit, !unavailable else { return }
    focus = nil
    Task {
      if resetting {
        if await store.resetPassword(email: email, code: code, password: password) {
          password = ""
          confirmation = ""
          code = ""
          onSwitch()
        }
      } else {
        let success = await store.authenticate(
          username: username, email: email, password: password,
          register: registering, verificationCode: code)
        if success {
          password = ""
          code = ""
          onAuthenticated()
        }
      }
    }
  }
}
