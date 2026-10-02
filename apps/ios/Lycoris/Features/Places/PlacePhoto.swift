import SwiftUI

/// Public photos load without cookies; private photos arrive from the account-scoped store.
struct PlacePhoto: View {
  let place: PlacePresentation
  var authenticated = false
  var data: Data? = nil
  var photoFailed = false
  var loadPrivatePhoto: ((URL) async throws -> Data)? = nil
  @State private var loadedImage: UIImage?
  @State private var failed = false

  var body: some View {
    Group {
      if let asset = place.photoAsset {
        Image(asset).resizable().aspectRatio(353.0 / 198, contentMode: .fit)
      } else if let loadedImage = authenticated && loadPrivatePhoto == nil
        ? data.flatMap(UIImage.init(data:)) : loadedImage
      {
        Rectangle().fill(.clear).aspectRatio(353.0 / 198, contentMode: .fit)
          .overlay { Image(uiImage: loadedImage).resizable().scaledToFill() }.clipped()
      } else if place.imageURL != nil {
        Rectangle().fill(.quaternary).aspectRatio(353.0 / 198, contentMode: .fit)
          .overlay {
            if failed || photoFailed || (authenticated && data != nil) {
              Text("Photo unavailable").font(.subheadline).foregroundStyle(.secondary)
            } else {
              ProgressView()
            }
          }
      }
    }
    .clipShape(RoundedRectangle(cornerRadius: 16))
    .accessibilityLabel("Place photo")
    .task(id: PhotoRequest(url: place.imageURL, authenticated: authenticated)) {
      loadedImage = nil
      failed = false
      guard let url = place.imageURL else { return }
      do {
        if authenticated {
          guard let loadPrivatePhoto else { return }
          let bytes = try await loadPrivatePhoto(url)
          try Task.checkCancellation()
          guard let image = UIImage(data: bytes) else { throw PlaceFailure.invalidResponse }
          loadedImage = image
        } else {
          loadedImage = try await PlaceImageLoader.load(url)
        }
      } catch {
        if !Task.isCancelled { failed = true }
      }
    }
  }

  private struct PhotoRequest: Hashable {
    let url: URL?
    let authenticated: Bool
  }
}

@MainActor
struct PlaceImageLoader {
  static func load(_ url: URL, session injectedSession: URLSession? = nil) async throws -> UIImage {
    let configuration = URLSessionConfiguration.ephemeral
    configuration.httpShouldSetCookies = false
    configuration.httpCookieStorage = nil
    configuration.urlCredentialStorage = nil
    configuration.timeoutIntervalForRequest = 15
    let session = injectedSession ?? URLSession(configuration: configuration)
    defer { if injectedSession == nil { session.finishTasksAndInvalidate() } }
    let (data, response) = try await session.data(from: url)
    try Task.checkCancellation()
    guard (response as? HTTPURLResponse)?.statusCode == 200,
      let image = UIImage(data: data)
    else { throw PlaceFailure.invalidResponse }
    return image
  }
}

struct PlaceShareSheet: UIViewControllerRepresentable {
  let place: PlacePresentation
  func makeUIViewController(context: Context) -> UIActivityViewController {
    let items: [Any] = [place.title] + (place.shareURL.map { [$0] } ?? [])
    return UIActivityViewController(activityItems: items, applicationActivities: nil)
  }
  func updateUIViewController(_ controller: UIActivityViewController, context: Context) {}
}
