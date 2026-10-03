import PhotosUI
import SwiftUI

struct ContributionSheet: View {
  @Bindable var store: ContributionStore
  var editID: Int64? = nil
  var onPickLocation: () -> Void
  @Environment(\.dismiss) private var dismiss
  @State private var photos: [PhotosPickerItem] = []
  @State private var photoLoading = false
  @State private var photoError: String?
  @State private var confirmsDiscard = false
  @State private var confirmsResend = false
  @State private var editLoadAttempt = UUID()
  @State private var editLoadError: String?
  @FocusState private var focused: Bool

  var body: some View {
    NavigationStack {
      Form {
        if let draft = store.draft {
          if draft.phase == .complete {
            Section {
              Label("Submitted for review", systemImage: "checkmark.circle")
                .accessibilityIdentifier("contribution.complete")
              Text("Your contribution will appear after it is approved.")
                .foregroundStyle(.secondary)
            }
          } else {
            locationSection(draft)
            fields(draft)
            Section {
              if let bytes = store.photoPreview, let image = UIImage(data: bytes) {
                Image(uiImage: image).resizable().scaledToFit()
                  .frame(maxHeight: 200).accessibilityLabel("Selected photo")
              }
              if draft.editable || draft.photoRejected {
                PhotosPicker(
                  selection: $photos, maxSelectionCount: draft.photoRejected ? 1 : nil,
                  selectionBehavior: .ordered, matching: .images
                ) {
                  Label(
                    draft.photoRejected ? "Replace photo" : "Add photos",
                    systemImage: "photo")
                }
                .accessibilityIdentifier("contribution.photo")
                .disabled(store.isWorking || photoLoading)
                ForEach(Array(draft.remainingPhotos.enumerated()), id: \.element.id) {
                  index, item in
                  HStack {
                    Text("\(index + 1)").monospacedDigit()
                    Text("Selected photo")
                    Spacer()
                    if draft.editable {
                      Button("Remove photo", role: .destructive) {
                        do { try store.removeQueuedPhoto(item.id) } catch { showPhotoError() }
                      }.disabled(store.isWorking || photoLoading)
                    }
                  }
                }
              }
              if photoLoading { ProgressView("Preparing photo…") }
              if let photoError { Text(photoError).foregroundStyle(.secondary) }
              if draft.phase == .uploading, let upload = draft.upload {
                ProgressView(value: Double(upload.receivedBytes), total: Double(upload.totalBytes))
                  .accessibilityLabel("Photo upload")
              }
            } footer: {
              Text(
                "Photos are submitted for review. Interrupted uploads resume when you return to the app."
              )
            }
            if draft.phase == .uncertainEdit {
              Section {
                Text("The edit could not be confirmed. It may already be awaiting review.")
                Button("Resend edit…") { confirmsResend = true }
                  .disabled(store.isWorking)
              }
            } else if let message = store.message {
              Section {
                Text(message).foregroundStyle(.secondary)
                if !draft.editable && !draft.photoRejected {
                  Button("Try again") { store.retry() }.disabled(store.isWorking)
                }
              }
            }
            if store.isWorking { Section { ProgressView("Submitting…") } }
            Section {
              Button("Discard saved contribution…", role: .destructive) { confirmsDiscard = true }
                .accessibilityIdentifier("contribution.discard")
            }.disabled(store.isWorking || photoLoading)
          }
        } else if let editLoadError {
          Section {
            Text(editLoadError).foregroundStyle(.secondary)
            Button("Try again") { editLoadAttempt = UUID() }
              .accessibilityIdentifier("contribution.retryLoad")
          }
        } else if editID != nil {
          Section { ProgressView().accessibilityIdentifier("contribution.loading") }
        }
      }
      .navigationTitle(editID != nil || store.draft?.original != nil ? "Edit place" : "Contribute")
      .navigationBarTitleDisplayMode(.inline)
      .scrollDismissesKeyboard(.interactively)
      .task(id: editLoadAttempt) {
        guard let editID else { return }
        editLoadError = nil
        do {
          try await store.edit(editID)
        } catch {
          guard !Task.isCancelled, !(error is CancellationError) else { return }
          editLoadError =
            (error as? AccountFailure)?.message
            ?? String(appLocalized: "Could not load places. Please try again.")
        }
      }
      .toolbar {
        ToolbarItem(placement: .cancellationAction) {
          Button("Close", systemImage: "xmark") {
            dismiss()
          }
          .accessibilityIdentifier("contribution.close")
        }
        if store.draft?.editable == true {
          ToolbarItem(placement: .confirmationAction) {
            Button("Submit", systemImage: "checkmark") {
              focused = false
              store.submit()
            }
            .buttonStyle(.borderedProminent)
            .disabled(store.draft?.canSubmit != true || store.isWorking || photoLoading)
            .accessibilityIdentifier("contribution.submit")
          }
        }
        ToolbarItemGroup(placement: .keyboard) {
          Spacer()
          Button("Done") { focused = false }
        }
      }
      .confirmationDialog(
        "Discard this saved contribution?", isPresented: $confirmsDiscard, titleVisibility: .visible
      ) {
        Button("Discard", role: .destructive) {
          do {
            try store.discard()
            dismiss()
          } catch { showPhotoError() }
        }
      } message: {
        Text(
          "This removes the local draft and photo. Anything already submitted remains awaiting review."
        )
      }
      .confirmationDialog(
        "Resend this edit?", isPresented: $confirmsResend, titleVisibility: .visible
      ) {
        Button("Resend edit") {
          do { try store.resendEdit() } catch { showPhotoError() }
        }
      } message: {
        Text(
          "The previous submission may have succeeded. Resending can create a duplicate proposal.")
      }
      .task(id: photos) {
        guard !photos.isEmpty, let draftID = store.draft?.id else { return }
        photoLoading = true
        photoError = nil
        defer {
          photoLoading = false
          // A later picker visit must not import the previous selection again.
          photos = []
        }
        do {
          for photo in photos {
            guard let bytes = try await photo.loadTransferable(type: Data.self) else {
              throw ContributionFailure.missingPhoto
            }
            try Task.checkCancellation()
            let encoded = try AvatarEncoder.jpeg(from: bytes, maximumDimension: 2048)
            guard draftID == store.draft?.id else { return }
            if store.draft?.photoRejected == true {
              try store.choosePhoto(encoded)
            } else {
              try store.addPhoto(encoded)
            }
          }
        } catch { if !Task.isCancelled { showPhotoError() } }
      }
    }
  }

  private func locationSection(_ draft: ContributionDraft) -> some View {
    Section {
      if draft.original == nil && draft.editable {
        Button(action: onPickLocation) {
          Label {
            Text("Choose another location on the map", tableName: "ContributionLocation")
          } icon: {
            Image(systemName: "mappin.and.ellipse")
          }
        }
        .accessibilityIdentifier("contribution.location")
        .accessibilityValue(
          String(format: "%.5f, %.5f", draft.point.latitude, draft.point.longitude))
      }
      LabeledContent("Location") {
        Text(
          "\(draft.point.latitude.formatted(.number.precision(.fractionLength(5)))), \(draft.point.longitude.formatted(.number.precision(.fractionLength(5))))"
        )
        .monospacedDigit()
      }
    }
    .disabled(store.isWorking || photoLoading)
  }

  @ViewBuilder private func fields(_ draft: ContributionDraft) -> some View {
    Group {
      Section {
        TextField("Title", text: field(\.title)).focused($focused)
          .submitLabel(.done).onSubmit { focused = false }
          .accessibilityIdentifier("contribution.title")
        ForEach(PlaceCategory.allCases, id: \.self) { category in
          HStack {
            Toggle(
              category.title,
              isOn: Binding(
                get: { draft.fields.selectedCategories.contains(category) },
                set: { selected in
                  var fields = draft.fields
                  let values =
                    selected
                    ? fields.selectedCategories + [category]
                    : fields.selectedCategories.filter { $0 != category }
                  fields.selectCategories(values)
                  store.update(fields)
                })
            ).accessibilityIdentifier("contribution.category.\(category.rawValue)")
            if draft.fields.selectedCategories.contains(category),
              draft.fields.selectedCategories.first != category
            {
              Button("Make primary") {
                var fields = draft.fields
                fields.selectCategories(
                  [category] + fields.selectedCategories.filter { $0 != category })
                store.update(fields)
              }.font(.caption).buttonStyle(.borderless)
                .accessibilityIdentifier("contribution.primary.\(category.rawValue)")
            }
          }
        }
        if draft.fields.selectedCategories.contains(.toilet) {
          Picker(
            selection: venueSelection(),
            label: Text(
              String(
                appLocalized: "Venue type", language: AppLanguage.current(),
                table: "PlaceMetadata"))
          ) {
            // A missing/unknown tag is a non-actionable placeholder, not a
            // false "Other" selection. Only recognized options can be chosen.
            Text(venuePlaceholder())
              .tag(PlaceVenue?.none)
              .disabled(true)
            ForEach(PlaceVenue.allCases) { venue in
              Text(venueTitle(venue)).tag(PlaceVenue?.some(venue))
            }
          }
          .accessibilityIdentifier("contribution.venue")
          .accessibilityValue(
            draft.fields.venueType.map(venueTitle) ?? venuePlaceholder())
        }
        TextField("Description", text: field(\.description), axis: .vertical)
          .lineLimit(3...8).focused($focused).accessibilityIdentifier("contribution.description")
      }
      Section {
        if draft.fields.openTimeStart.isEmpty {
          Button("Add opening hours") {
            var fields = draft.fields
            fields.openTimeStart = "09:00"
            fields.openTimeEnd = "18:00"
            fields.closingDayOverride = nil
            store.update(fields)
          }
        } else {
          DatePicker(
            "Opening Time", selection: time(\.openTimeStart), displayedComponents: .hourAndMinute)
            .accessibilityIdentifier("contribution.opening-time")
          DatePicker(
            "Closing Time", selection: time(\.openTimeEnd), displayedComponents: .hourAndMinute)
            .accessibilityIdentifier("contribution.closing-time")
          Toggle(isOn: closingDay()) {
            Text("Closes the next day", tableName: "OpeningHours")
          }
          .accessibilityIdentifier("contribution.next-day")
          if draft.fields.openingHoursMatchClosingDay {
            Text(PlacePresentation.hours(start: draft.fields.openTimeStart, end: draft.fields.openTimeEnd))
              .foregroundStyle(.secondary)
              .accessibilityIdentifier("contribution.hours-preview")
          }
          Button("Remove opening hours", role: .destructive) {
            var fields = draft.fields
            fields.openTimeStart = ""
            fields.openTimeEnd = ""
            fields.closingDayOverride = nil
            store.update(fields)
          }
        }
        TextField(
          "Opening hours note",
          text: Binding(
            get: { draft.fields.openingHoursNote ?? "" },
            set: { value in
              var fields = draft.fields
              fields.openingHoursNote = value
              store.update(fields)
            }), axis: .vertical
        )
        .lineLimit(2...5).accessibilityIdentifier("contribution.hours-note")
      } footer: {
        if !draft.fields.openTimeStart.isEmpty {
          if !draft.fields.openingHoursMatchClosingDay {
            Text(draft.fields.closesNextDay
              ? "For next-day closing, choose a time earlier than opening. Matching times mean open 24 hours."
              : "Closing is earlier than opening. Turn on next-day closing, or choose a later time.",
              tableName: "OpeningHours")
              .accessibilityIdentifier("contribution.hours-error")
          } else {
            Text("Matching opening and closing times mean open 24 hours.")
          }
        }
      }
    }.disabled(!draft.editable || store.isWorking || photoLoading)
  }

  /// The category binding routes through `switchingCategory`, so an explicit
  /// move away from a toilet clears the tag and a move back restores the other
  /// default without disturbing an ordinary text/time edit.
  private func categorySelection() -> Binding<PlaceCategory> {
    Binding(
      get: { store.draft?.fields.category ?? .toilet },
      set: { category in
        guard var fields = store.draft?.fields else { return }
        let previous = fields.category
        fields.category = category
        fields = ContributionStore.switchingCategory(fields, from: previous)
        store.update(fields)
      })
  }

  /// The Picker binds through the normalized `store.update`, so switching
  /// category away and back clears any stale tag and restores the `other`
  /// default. Selecting the current value still routes through the setter.
  private func venueSelection() -> Binding<PlaceVenue?> {
    Binding(
      get: { store.draft?.fields.venueType },
      set: { venue in
        guard var fields = store.draft?.fields, let venue else { return }
        fields.venueType = venue
        fields.unknownVenueType = nil
        store.update(fields)
      })
  }

  /// The disabled placeholder shown when a toilet has no known venue yet.
  private func venuePlaceholder() -> String {
    String(
      appLocalized: "Not specified", language: AppLanguage.current(), table: "PlaceMetadata")
  }

  private func venueTitle(_ venue: PlaceVenue) -> String {
    venue.title(language: AppLanguage.current())
  }

  private func field<Value>(_ key: WritableKeyPath<ContributionFields, Value>) -> Binding<Value> {
    Binding(
      get: { (store.draft?.fields ?? ContributionFields(language: "en"))[keyPath: key] },
      set: { value in
        guard var fields = store.draft?.fields else { return }
        fields[keyPath: key] = value
        store.update(fields)
      })
  }

  private func time(_ key: WritableKeyPath<ContributionFields, String>) -> Binding<Date> {
    Binding(
      get: {
        let parts = (store.draft?.fields[keyPath: key] ?? "09:00").split(separator: ":").compactMap
        { Int($0) }
        return Calendar.current.date(
          from: DateComponents(
            year: 2001, month: 1, day: 1,
            hour: parts.first ?? 9, minute: parts.last ?? 0)) ?? Date()
      },
      set: { date in
        guard var fields = store.draft?.fields else { return }
        let components = Calendar.current.dateComponents([.hour, .minute], from: date)
        fields[keyPath: key] = String(
          format: "%02d:%02d", components.hour ?? 0, components.minute ?? 0)
        store.update(fields)
      })
  }

  private func closingDay() -> Binding<Bool> {
    Binding(
      get: { store.draft?.fields.closesNextDay ?? false },
      set: { nextDay in
        guard var fields = store.draft?.fields else { return }
        fields.closingDayOverride = nextDay
        store.update(fields)
      })
  }

  private func showPhotoError() {
    photoError = String(
      appLocalized: "Could not prepare or save the photo. Please try another image.")
  }
}
