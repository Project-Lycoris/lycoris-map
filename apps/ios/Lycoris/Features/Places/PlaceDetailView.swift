import SwiftUI

struct PlaceDetailView: View {
  @Environment(\.dynamicTypeSize) private var dynamicTypeSize
  @Environment(\.horizontalSizeClass) private var horizontalSizeClass
  @Environment(\.lycorisMetadataNow) private var metadataNow
  let place: PlacePresentation
  let bottomInset: CGFloat
  var state: PlaceStore.LoadState = .idle
  var onRetry: () -> Void = {}
  var onShare: () -> Void = {}
  var onNavigate: () -> Void = {}
  var onEdit: () -> Void = {}
  var isBookmarked = false
  var bookmarkBusy = false
  var onBookmark: (() -> Void)? = nil
  var authenticatedPhoto = false
  var photo: Data? = nil
  var photoFailed = false
  var loadPrivatePhoto: ((URL) async throws -> Data)? = nil
  var reportsContentHeight = true
  var onContentHeight: (CGFloat) -> Void = { _ in }
  let onUnavailableAction: () -> Void
  @State private var photoIndex = 0
  @AccessibilityFocusState private var titleFocused: Bool
  @ScaledMetric(relativeTo: .body) private var buttonHeight: CGFloat = 48

  private struct ContentMeasurement: Equatable {
    let id: String
    let height: CGFloat
    let isResting: Bool
  }

  var body: some View {
    ScrollView {
      VStack(alignment: .leading, spacing: 11) {
        // Let compact titles use the button's transparent leading space, keeping
        // 11pt to the visible icon and the full 44pt button frame for hit testing.
        HStack(alignment: .center, spacing: horizontalSizeClass == .compact ? 11 - (44 - 20) : 11) {
          Text(place.detailTitle).font(.title3.weight(.semibold))
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityIdentifier("place.title")
            .accessibilityAddTraits(.isHeader).accessibilityFocused($titleFocused)
          Button(action: onEdit) {
            // Template rendering follows the label's primary foreground so the
            // black SVG stays visible in dark mode.
            Image("PlaceEdit").renderingMode(.template).resizable().frame(width: 20, height: 20)
              .foregroundStyle(.primary)
              .frame(width: 44, height: 44, alignment: .trailing)
              .contentShape(Rectangle())
          }
          .buttonStyle(.plain).accessibilityLabel("Edit place")
          .accessibilityIdentifier("place.edit")
        }

        if !hasFailed, place.venue != nil || placeClosingSoon {
          let tagLayout =
            dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 11))
            : AnyLayout(HStackLayout(spacing: 11))
          tagLayout {
            if let venue = place.venue { PlaceVenueTag(venue: venue) }
            PlaceClosingSoonTag(place: place)
          }
        }

        if !hasFailed, place.categories.count > 1 {
          ViewThatFits(in: .horizontal) {
            HStack { facilityTags }
            VStack(alignment: .leading) { facilityTags }
          }
        }
        if state == .loading || hasFailed {
          PlaceLoadStatus(state: state, spacing: 11, horizontalInset: 0, retry: onRetry)
        }
        if !hasFailed {
          let metadataLayout =
            dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: 11))
            : AnyLayout(HStackLayout(spacing: 16))
          metadataLayout {
            if !place.distance.isEmpty { Text(place.distance) }
            PlaceOpeningHoursView(place: place, suppressesClosingSoonText: placeClosingSoon)
          }
          .font(.subheadline).foregroundStyle(.secondary)

          if let reference = place.distanceReference {
            Text(reference).font(.caption).foregroundStyle(.secondary)
          }
          if let note = place.openingHoursNote, !note.isEmpty {
            Text(note).font(.subheadline).fixedSize(horizontal: false, vertical: true)
          }
          if !place.description.isEmpty {
            Text(place.description).font(.subheadline).foregroundStyle(.secondary)
              .fixedSize(horizontal: false, vertical: true)
          }
          if place.hasPhoto {
            PlacePhoto(
              place: galleryPlace, authenticated: authenticatedPhoto, data: photo,
              photoFailed: photoFailed, loadPrivatePhoto: loadPrivatePhoto
            )
            if place.imageURLs.count > 1 {
              HStack {
                Button("Previous photo", systemImage: "chevron.left") { photoIndex -= 1 }.disabled(
                  photoIndex == 0
                ).labelStyle(.iconOnly)
                Spacer()
                Text("\(photoIndex + 1) / \(place.imageURLs.count)").font(.caption)
                Spacer()
                Button("Next photo", systemImage: "chevron.right") { photoIndex += 1 }.disabled(
                  photoIndex >= place.imageURLs.count - 1
                ).labelStyle(.iconOnly)
              }
            }
          }

          let actionLayout =
            dynamicTypeSize.isAccessibilitySize
            ? AnyLayout(VStackLayout(spacing: 11)) : AnyLayout(HStackLayout(spacing: 14))
          actionLayout {
            actionLabel(
              "Share", image: "PlaceShare", size: 20, identifier: "place.share", action: onShare
            )
            .buttonStyle(.glass)
            actionLabel(
              "Navigate", image: "PlaceNavigate", size: 24, identifier: "place.navigate",
              action: onNavigate
            )
            .buttonStyle(.glassProminent)
            Button(action: onBookmark ?? onUnavailableAction) {
              Group {
                if isBookmarked {
                  Image(systemName: "bookmark.fill").resizable().scaledToFit()
                    .foregroundStyle(.blue)
                } else {
                  // Template rendering follows the label's primary foreground so
                  // the black SVG stays visible in dark mode.
                  Image("PlaceBookmark").renderingMode(.template).resizable()
                    .foregroundStyle(.primary)
                }
              }.frame(width: 28, height: 28)
                .frame(minWidth: 44, minHeight: buttonHeight)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain).accessibilityLabel(
              isBookmarked ? "Remove bookmark" : "Bookmark place"
            )
            .accessibilityIdentifier("place.bookmark").disabled(bookmarkBusy)
          }
        }
      }
      .padding(.horizontal, 11)
      .padding(.top, 11)
      .padding(.bottom, max(bottomInset, 11))
      .onGeometryChange(for: ContentMeasurement.self) {
        ContentMeasurement(id: place.id, height: $0.size.height, isResting: reportsContentHeight)
      } action: {
        if $0.isResting { onContentHeight($0.height) }
      }
    }
    .scrollIndicators(.hidden)
    .ignoresSafeArea(.container, edges: .bottom)
    .accessibilityIdentifier("place.details")
    .task(id: place.id) {
      titleFocused = true
      photoIndex = 0
    }
  }

  private var galleryPlace: PlacePresentation {
    var value = place
    if !place.imageURLs.isEmpty {
      value.imageURL = place.imageURLs[min(photoIndex, place.imageURLs.count - 1)]
    }
    return value
  }
  private var facilityTags: some View {
    ForEach(place.categories, id: \.self) { category in
      Text(category.title).font(.caption.weight(.medium)).padding(.horizontal, 10).padding(
        .vertical, 5
      )
      .background(Color(category.tint).opacity(0.18), in: Capsule())
    }
  }

  private var hasFailed: Bool {
    if case .failed = state { return true }
    return false
  }

  private var placeClosingSoon: Bool {
    place.openingStatus(at: metadataNow) == .closingSoon
  }

  private func actionLabel(
    _ title: LocalizedStringKey, image: String, size: CGFloat, identifier: String,
    action: @escaping () -> Void
  ) -> some View {
    Button(action: action) {
      HStack(spacing: 6) {
        Text(title).font(.body.weight(.medium))
          .lineLimit(1).minimumScaleFactor(0.85)
        // Template rendering inherits the button style's foreground: primary on
        // the glass Share button, white on the prominent blue Navigate button.
        Image(image).renderingMode(.template).resizable().frame(width: size, height: size)
      }
      .frame(maxWidth: .infinity, minHeight: max(32, buttonHeight - 16))
      .contentShape(Capsule())
    }
    .buttonBorderShape(.capsule)
    .accessibilityIdentifier(identifier)
  }
}
