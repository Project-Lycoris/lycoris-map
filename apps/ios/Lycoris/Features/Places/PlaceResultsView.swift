import SwiftUI

struct PlaceResultsView: View {
  let store: PlaceStore
  let onSelect: (PlacePresentation) -> Void
  let onClose: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: 8) {
      HStack {
        Text(store.browse.isNearby || store.pendingNearby != nil ? "Nearby" : "Search results")
          .font(.title3.weight(.semibold)).accessibilityIdentifier("places.results.title")
          .accessibilityAddTraits(.isHeader)
        Spacer()
        Button(action: onClose) {
          Image(systemName: "xmark").font(.body.weight(.medium))
            .frame(width: 44, height: 44).contentShape(Rectangle())
        }.buttonStyle(.plain).accessibilityLabel("Close results")
      }.padding(.leading, 6)
      if case .nearby(let category, _, let located) = store.browse {
        Text(category.title + " · " + AppPreferences.radiusLabel(store.radius)).font(
          .body.weight(.semibold)
        ).padding(.horizontal, 6)
        Text(located ? "Around your location" : "Around map center")
          .font(.subheadline).foregroundStyle(.secondary).padding(.horizontal, 6)
      }
      PlaceLoadStatus(
        state: store.resultsState, empty: store.results.isEmpty, retry: store.retryResults)
      if !store.resultPlaces.isEmpty {
        LazyVStack(spacing: 0) {
          ForEach(store.resultPlaces) { place in
            PlaceRow(place: place) { onSelect(place) }
          }
        }
        .background(Color(.systemBackground), in: RoundedRectangle(cornerRadius: 24))
        .padding(.horizontal, 3)
      }
    }
    .accessibilityElement(children: .contain)
    .accessibilityIdentifier("places.results")
  }
}

extension Optional where Wrapped == PlaceStore.Browse {
  fileprivate var isNearby: Bool {
    if case .nearby = self { return true }
    return false
  }
}

struct PlaceLoadStatus: View {
  let state: PlaceStore.LoadState
  var empty = false
  var spacing: CGFloat = 8
  var horizontalInset: CGFloat = 6
  let retry: () -> Void

  var body: some View {
    VStack(alignment: .leading, spacing: spacing) {
      switch state {
      case .loading:
        HStack(spacing: 8) {
          ProgressView()
          Text("Loading places…")
        }
      case .failed(let error):
        Text(error.message)
        Button("Try again", action: retry).buttonStyle(.borderless)
      case .loaded where empty: Text("No places found.")
      default: EmptyView()
      }
    }
    .font(.subheadline).foregroundStyle(.secondary)
    .frame(maxWidth: .infinity, alignment: .leading).padding(.horizontal, horizontalInset)
    .accessibilityIdentifier("places.status")
  }
}
