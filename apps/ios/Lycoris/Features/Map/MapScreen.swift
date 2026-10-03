import AVFoundation
import MapKit
import SwiftUI

struct MapScreen: View {
  @State private var preferences = AppPreferences()
  @Namespace private var appearanceTransition
  @State private var locationDenied = false
  @State private var awaitsLocationAuthorization = false
  @State private var linkError = false
  @State private var voice = VoiceSearchController()
  @State private var showsVoiceSearch = false
  @State private var voiceTask: Task<Void, Never>?
  @Environment(\.openURL) private var openURL
  @Environment(\.dynamicTypeSize) private var dynamicTypeSize
  @State private var detent: MapPanelDetent
  @State private var sidebarDestination = MapSidebarDestination.search
  @State private var sidebarContentVisible = true
  @State private var store: PlaceStore
  @State private var location = LocationProvider()
  @State private var connectivity = ConnectivityMonitor()
  @State private var mapCoordinates: MapCoordinateResolver
  @State private var showsCoordinateError = false
  @State private var account = AccountStore()
  @State private var contribution = ContributionStore()
  @State private var measuredDetail: (id: String, height: CGFloat)?
  @State private var selectingLocation = false
  @State private var pickedLocation: GeoPoint?
  @State private var locationPickFeedback = 0
  @State private var screenCenter: GeoPoint?
  @State private var contributionIntent: ContributionIntent?
  @State private var queuedContribution: ContributionIntent?
  @State private var chooseLocationAfterDismiss = false
  @State private var contributionError: String?
  @State private var modal: MapModal?
  @State private var pendingBookmark: Int64?
  @State private var bookmarkIntent = UUID()
  @State private var showsAccountError = false
  @State private var dismissedPlaceFailure: PlaceFailure?
  @Environment(\.scenePhase) private var scenePhase
  /// One shared clock for every place's real-time opening status. All rows,
  /// details and sheets read this instead of each starting their own timer.
  @State private var metadataNow = Date()
  @State private var showsLocationError = false
  @State private var showsNavigationError = false
  private var selectedPlace: PlacePresentation? {
    account.selectedMarker.map { store.presentation($0) } ?? store.selectedPlace
  }
  private var placeFailure: PlaceFailure? {
    for state in [account.detailState, store.detailState, store.resultsState, store.viewportState] {
      if case .failed(let failure) = state { return failure }
    }
    return nil
  }
  private var mapPlaces: [PlacePresentation] {
    if account.detailState == .failed(.unavailable), let removed = account.selectedMarker {
      return store.mapPlaces.filter { $0.id != String(removed.id) }
    }
    guard let selected = account.selectedMarker.map({ store.presentation($0) }) else {
      return store.mapPlaces
    }
    return store.mapPlaces.filter { $0.id != selected.id } + [selected]
  }
  @State private var showsUnavailableAction = false
  private let bookmarks: [PlacePresentation]
  @GestureState private var dragTranslation: CGFloat = 0
  @State private var contentDragTop: CGFloat?
  @State private var panelGestureGeneration = 0
  @State private var query = ""
  @State private var keyboardHeight: CGFloat = 0
  @FocusState private var isSearchFocused: Bool
  @Environment(\.accessibilityReduceMotion) private var reduceMotion
  @Environment(\.accessibilityVoiceOverEnabled) private var voiceOverEnabled
  @ScaledMetric(relativeTo: .subheadline) private var searchHeight: CGFloat = 38
  @ScaledMetric(relativeTo: .body) private var cardHeight: CGFloat = 66
  @ScaledMetric(relativeTo: .title3) private var titleHeight: CGFloat = 24

  init(
    initialDetent: MapPanelDetent = .collapsed, bookmarks: [PlacePresentation] = [],
    initialPlace: PlacePresentation? = nil, isPreview: Bool = false
  ) {
    _detent = State(initialValue: initialDetent)
    _store = State(initialValue: PlaceStore(isPreview: isPreview, initialPlace: initialPlace))
    #if LYCORIS_LOCAL_TESTS
      // Hermetic fixture runs have an explicit datum. Live provider calibration is
      // verified separately, not allowed to make fixture tests depend on Apple search.
      let calibrationUnavailable = ProcessInfo.processInfo.arguments.contains(
        "-lycoris-test-map-calibration-unavailable")
      _mapCoordinates = State(
        initialValue: calibrationUnavailable
          ? MapCoordinateResolver(lookup: { nil }) : MapCoordinateResolver(space: .wgs84))
    #else
      _mapCoordinates = State(
        initialValue: MapCoordinateResolver(space: isPreview ? .wgs84 : nil))
    #endif
    self.bookmarks = bookmarks
  }

  /// The map/panel/tools surface. Kept as a separate opaque view so the very
  /// large modifier chain does not have to be type-checked as one expression
  /// together with the lifecycle, sheet and alert modifiers in `body`.
  private var mapSurface: some View {
    GeometryReader { geometry in
      let layout = PanelLayout(
        viewport: CGSize(
          width: geometry.size.width,
          height: geometry.size.height + geometry.safeAreaInsets.top
            + geometry.safeAreaInsets.bottom),
        topInset: geometry.safeAreaInsets.top,
        bottomInset: geometry.safeAreaInsets.bottom,
        headerHeight: max(44, searchHeight) + 58,
        nearbyContentHeight: titleHeight + 8 + cardHeight
          * (dynamicTypeSize.isAccessibilitySize ? 3 : 2) + 24,
        detailHeight: detailHeight(geometry: geometry),
        collapsedHeaderHeight: max(44, searchHeight) + 28
      )
      let adaptive = AdaptiveMapLayout(size: layout.viewport)
      let sidebarTop = max(layout.topInset, 12) + 8
      let sidebarHeight = max(
        120,
        layout.viewport.height - sidebarTop
          - max(layout.bottomInset + 12, keyboardHeight + 12))
      let panelTop = layout.clampedTop(contentDragTop ?? (layout.top(for: detent) + dragTranslation))
      let panelHeight = layout.height(at: panelTop)
      let panelShape = UnevenRoundedRectangle(
        topLeadingRadius: 26, bottomLeadingRadius: layout.bottomCornerRadius(at: panelTop),
        bottomTrailingRadius: layout.bottomCornerRadius(at: panelTop), topTrailingRadius: 26)
      let toolsVisible =
        !selectingLocation
        && (adaptive.usesSidebar || (panelTop > layout.topInset + 270 && !isSearchFocused))
      // Keep attribution fixed above the panel's lowest resting position.
      let mapBottomInset =
        adaptive.usesSidebar
        ? max(layout.bottomInset + 12, keyboardHeight + 12)
        : layout.viewport.height - layout.collapsedTop + 10
      let showsUserLocation =
        !store.isPreview && location.hasRequestedLocation && location.isAuthorized
      let isActive = scenePhase == .active

      ZStack(alignment: .topLeading) {
        NativeMapView(
          topInset: layout.topInset, bottomInset: mapBottomInset,
          leftInset: selectingLocation
            ? 10
            : adaptive.leadingOcclusion(contentVisible: sidebarContentVisible),
          rightInset: adaptive.usesSidebar ? 80 : 10,
          appearance: preferences.mapAppearance,
          coordinateSpace: mapCoordinates.space,
          places: mapPlaces, language: preferences.language, focus: store.focus,
          showsUserLocation: showsUserLocation, isActive: isActive, animated: !reduceMotion,
          isSelectingLocation: selectingLocation, selectedLocation: pickedLocation,
          metadataNow: metadataNow,
          onPickLocation: pickLocation,
          onUnresolvedCoordinate: coordinateUnavailable,
          onViewport: { store.viewportChanged($0) },
          onSelect: { if !selectingLocation { selectPlace($0) } },
          onScreenCenter: { screenCenter = $0 }
        )
        .accessibilityIdentifier("map.canvas")
        .accessibilityHidden(!adaptive.usesSidebar && detent == .expanded && !selectingLocation)

        MapTools(
          spacing: selectedPlace == nil ? 23 : 10,
          locate: locate,
          showNearby: { showNearby(.toilet) },
          contribute: { beginContribution(.create) },
          showMapAppearance: { modal = .mapAppearance(screenCenter) },
          appearanceTransition: appearanceTransition
        )
        .position(
          x: layout.viewport.width - 40,
          y: adaptive.usesSidebar
            ? layout.topInset + 120
            : panelTop - (selectedPlace == nil ? 131.5 : 116)
        )
        .opacity(toolsVisible ? 1 : 0)
        .allowsHitTesting(toolsVisible)
        .accessibilityHidden(!toolsVisible)

        if adaptive.usesSidebar {
          MapSidebar(
            showsLabels: adaptive.showsNavigationLabels,
            selection: sidebarDestination, contentVisible: sidebarContentVisible,
            user: account.user, avatar: account.avatar,
            onSearch: {
              openSidebar(.search)
              Task { @MainActor in
                await Task.yield()
                guard !selectingLocation, modal == nil else { return }
                isSearchFocused = true
              }
            },
            onBookmarks: { openSidebar(.bookmarks) },
            onContribute: { beginContribution(.create) },
            onSettings: {
              isSearchFocused = false
              modal = .settingsHome
            },
            onAccount: {
              isSearchFocused = false
              modal = .account(.profile)
            }
          )
          .frame(width: adaptive.navigationWidth, height: sidebarHeight)
          .position(
            x: adaptive.spacing + adaptive.navigationWidth / 2,
            y: sidebarTop + sidebarHeight / 2
          )
          .opacity(selectingLocation ? 0 : 1)
          .disabled(selectingLocation)
          .allowsHitTesting(!selectingLocation)
          .accessibilityHidden(selectingLocation)

          if sidebarContentVisible {
            sidebarContent
              .frame(width: adaptive.contentWidth, height: sidebarHeight)
              .clipShape(RoundedRectangle(cornerRadius: 26))
              .modifier(
                MapPanelSurface(
                  shape: UnevenRoundedRectangle(
                    cornerRadii: .init(
                      topLeading: 26, bottomLeading: 26, bottomTrailing: 26, topTrailing: 26)))
              )
              .position(
                x: adaptive.spacing * 2 + adaptive.navigationWidth + adaptive.contentWidth / 2,
                y: sidebarTop + sidebarHeight / 2
              )
              .opacity(selectingLocation ? 0 : 1)
              .disabled(selectingLocation)
              .allowsHitTesting(!selectingLocation)
              .accessibilityHidden(selectingLocation)
          }
        } else {
          panel(layout: layout, height: panelHeight)
            .frame(
              width: layout.viewport.width - layout.horizontalInset(at: panelTop) * 2,
              height: panelHeight
            )
            .clipShape(panelShape)
            .modifier(MapPanelSurface(shape: panelShape))
            .position(x: layout.viewport.width / 2, y: panelTop + panelHeight / 2)
            .opacity(selectingLocation ? 0 : 1)
            .allowsHitTesting(!selectingLocation)
            .accessibilityHidden(selectingLocation)

          grabberButton(layout: layout, panelTop: panelTop)
            .position(x: layout.viewport.width / 2, y: layout.grabberCenterY(at: panelTop))
            .opacity(selectingLocation ? 0 : 1)
            .allowsHitTesting(!selectingLocation)
            .accessibilityHidden(selectingLocation)
        }

        if selectingLocation {
          VStack {
            HStack {
              Button("Cancel") {
                selectingLocation = false
                pickedLocation = nil
                if contribution.draft?.editable == true { modal = .contribution() }
              }
              .buttonStyle(.glass).controlSize(.large)
              .accessibilityIdentifier("contribution.cancel-location")
              Spacer()
              Button("Current location", systemImage: "location", action: locate)
                .buttonStyle(.glass).controlSize(.large).labelStyle(.iconOnly)
            }
            Spacer()
            VStack(spacing: 12) {
              Text("Tap the map to choose a location.")
                .multilineTextAlignment(.center)
                .accessibilityIdentifier("contribution.pick-location")
              if voiceOverEnabled {
                Button("Select map center") {
                  if let screenCenter { pickLocation(screenCenter) }
                }
                .disabled(screenCenter == nil)
              }
            }
            .padding().frame(maxWidth: .infinity).background(
              .regularMaterial, in: .rect(cornerRadius: 26))
          }
          .padding(.horizontal).padding(.top, layout.topInset + 8)
          .padding(.bottom, max(layout.bottomInset, 16))
          .frame(width: layout.viewport.width, height: layout.viewport.height)
        }
      }
      .frame(width: layout.viewport.width, height: layout.viewport.height)
      .offset(y: -geometry.safeAreaInsets.top)
      .onChange(of: geometry.size) { _, _ in
        measuredDetail = nil
        cancelPanelDrag()
      }
      .onChange(of: adaptive.usesSidebar) { _, wide in
        cancelPanelDrag()
        guard !wide else { return }
        if !sidebarContentVisible {
          sidebarDestination = .search
          detent = .collapsed
        } else if isSearchFocused || showsVoiceSearch || sidebarDestination == .bookmarks {
          detent = .expanded
        } else if selectedPlace != nil {
          detent = .nearby
        }
      }
    }
  }

  /// The map surface plus its presentation modifiers (sheets, alerts, links).
  /// Kept separate from the lifecycle/observation modifiers so no single SwiftUI
  /// expression grows beyond what the type-checker can handle.
  private var mapPresentations: some View {
    mapSurface
      .ignoresSafeArea(.keyboard)
      .sensoryFeedback(.selection, trigger: locationPickFeedback)
      .alert(Text("Map unavailable", tableName: "Coordinates"), isPresented: $showsCoordinateError)
    {
      Button("Try again") { mapCoordinates.resolveIfNeeded(retryPending: true) }
      Button("Cancel", role: .cancel) {}
    } message: {
      if mapCoordinates.space == .unresolved {
        Text(
          "Connect to the internet and try again before choosing a map location.",
          tableName: "Coordinates")
      } else {
        Text(
          "This map location could not be verified. Please choose a nearby point.",
          tableName: "Coordinates")
      }
    }
      .alert("Not available yet", isPresented: $showsUnavailableAction) {
        Button("OK", role: .cancel) {}
      }
      .alert("Location unavailable", isPresented: $showsLocationError) {
        if locationDenied {
          Button("Open Settings") { openURL(URL(string: UIApplication.openSettingsURLString)!) }
        } else {
          Button("Try again", action: locate)
        }
        Button("Cancel", role: .cancel) {}
      } message: {
        Text(
          locationDenied
            ? "You can browse the map without location access. To use your location, enable it in Settings and try again."
            : "Could not get your location. You can try again or browse around the map center.")
      }
      .alert("Could not open Apple Maps", isPresented: $showsNavigationError) {
        Button("OK", role: .cancel) {}
      }
      .sheet(
        item: $modal,
        onDismiss: {
          pendingBookmark = nil
          bookmarkIntent = UUID()
          contributionIntent = nil
          if let intent = queuedContribution {
            queuedContribution = nil
            beginContribution(intent)
          } else if chooseLocationAfterDismiss {
            chooseLocationAfterDismiss = false
            enterLocationSelection(at: contribution.draft?.point)
          }
        }
      ) { item in
        switch item {
        case .share(let place): PlaceShareSheet(place: place)
        case .settingsHome: SettingsHomeSheet(preferences: preferences)
        case .settings(let destination):
          SettingsSheet(preferences: preferences, destination: destination)
        case .mapAppearance(let center):
          MapAppearanceSheet(
            preferences: preferences, center: center, coordinateSpace: mapCoordinates.space
          )
          .navigationTransition(.zoom(sourceID: "map-appearance", in: appearanceTransition))
        case .link(let link):
          PlaceLinkSheet(link: link, account: account) { marker, authenticated in
            guard case .link(let current) = modal, current == link else { return }
            modal = nil
            if authenticated {
              selectAccountPlace(marker)
            } else {
              selectPlace(store.presentation(marker))
            }
          }
        case .account(let destination):
          AccountSheet(
            store: account, destination: destination, contributions: contribution,
            onDraft: { id in
              do {
                try contribution.openDraft(id)
                modal = .contribution(editID: nil)
              } catch {
                contributionError = String(appLocalized: "Could not read the saved contribution.")
              }
            }, onAuthenticated: resumeAuthenticatedAction,
            onSelect: selectAccountPlace)
        case .contribution(let editID):
          ContributionSheet(store: contribution, editID: editID) {
            chooseLocationAfterDismiss = true
            modal = nil
          }
        }
      }
      .overlay(alignment: .top) {
        if let failure = placeFailure, dismissedPlaceFailure != failure {
          HStack(alignment: .top, spacing: 11) {
            Text(failure.message).font(.subheadline).frame(maxWidth: .infinity, alignment: .leading)
            Button("Close", systemImage: "xmark") { dismissedPlaceFailure = failure }.labelStyle(
              .iconOnly)
          }.padding(16).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 20))
            .padding(.horizontal, 22).padding(.top, 8).accessibilityIdentifier(
              "places.error-notice")
        }
      }
      .onChange(of: placeFailure) { _, _ in dismissedPlaceFailure = nil }
      .alert("Account", isPresented: $showsAccountError) {
        Button("OK", role: .cancel) {}
      } message: {
        Text(account.message ?? "")
      }
      .alert(
        "Contribute",
        isPresented: Binding(
          get: { contributionError != nil }, set: { if !$0 { contributionError = nil } })
      ) {
        Button("OK", role: .cancel) { contributionError = nil }
      } message: {
        Text(contributionError ?? "")
      }
      .onOpenURL { url in
        guard let link = PlaceLink(url: url), modal == nil, !selectingLocation else {
          linkError = true
          return
        }
        isSearchFocused = false
        modal = .link(link)
      }
      .alert("Could not open place link", isPresented: $linkError) {
        Button("OK", role: .cancel) {}
      } message: {
        Text("Check the link and close any open sheet before trying again.")
      }
  }

  private var mapLifecycle: some View {
    mapPresentations
      .onChange(of: preferences.language) { _, _ in
        cancelVoiceSearch()
        applyPreferences()
      }
      .onChange(of: preferences.radius) { _, _ in applyPreferences() }
      .onChange(of: preferences.searchType) { _, _ in applyPreferences() }
      .onChange(of: location.isAuthorized) { _, authorized in
        if !authorized {
          store.revokeLocation()
          awaitsLocationAuthorization = location.hasRequestedLocation
        } else {
          requestStartupLocation()
        }
      }
      .task {
        applyPreferences()
        requestStartupLocation()
        if !store.isPreview {
          mapCoordinates.resolveIfNeeded()
          connectivity.start()
          contribution.connect(account)
          await account.restore()
          contribution.synchronize()
        }
      }
      // A single minute-boundary clock for opening status. It restarts when the
      // scene becomes active and refreshes immediately on return to foreground;
      // it never triggers a network request or moves the map.
      .task(id: scenePhase) {
        await runMetadataClock()
      }
      .onChange(of: scenePhase) { _, phase in
        if phase != .active { cancelPanelDrag() }
        if phase == .background
          || (phase == .inactive && (voice.state == .recording || voice.state == .finishing))
        {
          cancelVoiceSearch()
        }
        guard !store.isPreview else { return }
        contribution.setActive(phase == .active)
        if phase == .active {
          mapCoordinates.resolveIfNeeded(retryPending: true)
          location.refreshAuthorization()
          if !location.isAuthorized { store.revokeLocation() }
          requestStartupLocation()
          store.retryFailedRequests()
          Task {
            await account.restore()
            contribution.synchronize()
          }
        }
      }
      .onChange(of: connectivity.recoveryCount) { _, _ in
        guard !store.isPreview, scenePhase == .active else { return }
        mapCoordinates.resolveIfNeeded(retryPending: true)
        store.networkDidRecover()
        Task {
          await account.networkDidRecover()
          contribution.synchronize()
        }
      }
      .onChange(of: account.epoch) { _, _ in
        contribution.synchronize()
        if account.user == nil {
          if sidebarDestination == .bookmarks { sidebarDestination = .search }
          selectingLocation = false
          pickedLocation = nil
          chooseLocationAfterDismiss = false
          if case .contribution = modal { modal = nil }
        }
      }
      .onChange(of: account.user?.publicId) { old, new in
        if old != nil, old != new {
          selectingLocation = false
          pickedLocation = nil
          chooseLocationAfterDismiss = false
          queuedContribution = nil
          if case .contribution = modal { modal = nil }
          if case .link = modal { modal = nil }
        }
      }
      .onChange(of: account.detailState) { _, state in
        if state == .failed(.unavailable), let id = account.selectedMarker?.id {
          store.removeUnavailable(id)
        }
      }
  }

  var body: some View {
    mapLifecycle
      .onChange(of: query) { _, text in
        if !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
          account.closeDetail()
          store.search(text)
        } else if case .search = store.browse {
          store.closeResults()
        }
      }
      .onDisappear {
        cancelVoiceSearch()
        store.stop()
        connectivity.stop()
        mapCoordinates.stop()
      }
      .onChange(of: isSearchFocused) { _, focused in
        if focused {
          sidebarContentVisible = true
          sidebarDestination = .search
          cancelVoiceSearch()
          movePanel(to: .expanded)
        }
      }
      .onChange(of: modal?.id) { _, modalID in
        if modalID != nil { cancelVoiceSearch() }
      }
      .onChange(of: voice.transcript) { _, text in
        if showsVoiceSearch { query = text }
      }
      .onChange(of: voice.state) { _, state in
        if showsVoiceSearch && state == .ready { finishVoiceSearch() }
      }
      .onReceive(NotificationCenter.default.publisher(for: AVAudioSession.interruptionNotification))
    {
      _ in cancelVoiceSearch()
    }
      .onReceive(NotificationCenter.default.publisher(for: AVAudioSession.routeChangeNotification))
    {
      notification in
      let raw = notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt
      if voice.state == .recording,
        raw == AVAudioSession.RouteChangeReason.oldDeviceUnavailable.rawValue
          || raw == AVAudioSession.RouteChangeReason.noSuitableRouteForCategory.rawValue
      {
        cancelVoiceSearch()
      }
    }
      .background {
        MapKeyboardObserver { height in
          if keyboardHeight != height { keyboardHeight = height }
        }
      }
      .environment(\.locale, preferences.language.locale)
      .environment(\.lycorisAppLanguage, preferences.language)
      .environment(\.lycorisMetadataNow, metadataNow)
  }

  /// Refreshes the shared opening-status clock on every minute boundary while
  /// the scene is active, and immediately on foreground return. Cancellation
  /// (scene change, view teardown) exits the loop without leaving a timer. It
  /// performs no network work and does not touch the map.
  private func runMetadataClock() async {
    guard scenePhase == .active else { return }
    let fixed = Self.fixedMetadataNow()
    if let fixed {
      metadataNow = fixed
      return
    }
    while !Task.isCancelled {
      let now = Date()
      metadataNow = now
      let interval = 60 - (now.timeIntervalSince1970.truncatingRemainder(dividingBy: 60))
      do {
        try await Task.sleep(for: .seconds(interval))
      } catch {
        return
      }
    }
  }

  /// A fixed test clock for reproducible closing-soon tests. Only a Debug build
  /// with local test support honors the launch argument; Release always uses
  /// the real current time.
  private static func fixedMetadataNow() -> Date? {
    #if DEBUG && LYCORIS_LOCAL_TESTS
      let arguments = ProcessInfo.processInfo.arguments
      guard let index = arguments.firstIndex(of: "-lycoris-test-metadata-now"),
        arguments.indices.contains(index + 1),
        let seconds = TimeInterval(arguments[index + 1]),
        seconds.isFinite
      else { return nil }
      return Date(timeIntervalSince1970: seconds)
    #else
      return nil
    #endif
  }

  /// Fit the actual wrapped text, 11pt gaps and bottom safe area. The estimate
  /// is used only until the ScrollView has measured its content once.
  private func detailHeight(geometry: GeometryProxy) -> CGFloat? {
    guard let selectedPlace else { return nil }
    if let measuredDetail, measuredDetail.id == selectedPlace.id {
      return PanelLayout.grabberRealHeight + measuredDetail.height
    }
    var height: CGFloat = 208
    if selectedPlace.hasPhoto {
      height += (geometry.size.width - 22) * 198 / 353
    }
    if selectedPlace.distanceReference != nil { height += 30 }
    if selectedPlace.venue != nil { height += 30 }
    height += max(geometry.safeAreaInsets.bottom, 29)
    return height
  }

  private func applyPreferences() {
    store.updatePreferences(
      language: preferences.language.rawValue, radius: preferences.radius,
      searchType: preferences.searchType)
    account.updateLanguage(preferences.language.rawValue)
  }

  private func openSidebar(_ destination: MapSidebarDestination) {
    cancelVoiceSearch()
    isSearchFocused = false
    store.closeDetail()
    account.closeDetail()
    sidebarDestination = destination
    sidebarContentVisible = true
  }

  private func closeSidebar() {
    cancelVoiceSearch()
    isSearchFocused = false
    sidebarContentVisible = false
    store.closeDetail()
    account.closeDetail()
  }

  private var sidebarTitle: Text {
    if selectedPlace != nil {
      Text("Place details")
    } else if sidebarDestination == .bookmarks {
      Text("Bookmarks")
    } else {
      Text("Search", tableName: "AdaptiveMap")
    }
  }

  private var sidebarContent: some View {
    NavigationStack {
      Group {
        if selectedPlace != nil {
          placeDetails(bottomInset: 12, reportsContentHeight: false)
        } else if sidebarDestination == .bookmarks && account.user != nil {
          AccountPlacesView(store: account, created: false, onSelect: selectAccountPlace)
            .scrollContentBackground(.hidden)
        } else {
          sidebarSearch
        }
      }
      .navigationTitle(sidebarTitle)
      .navigationBarTitleDisplayMode(.inline)
      .toolbar {
        ToolbarItem(placement: .topBarTrailing) {
          Button(action: closeSidebar) { Image(systemName: "xmark") }
            .accessibilityLabel(Text("Close panel", tableName: "AdaptiveMap"))
            .accessibilityIdentifier("map.sidebar.close")
            .keyboardShortcut(.escape, modifiers: [])
        }
      }
      .toolbarBackground(.hidden, for: .navigationBar)
      .containerBackground(.clear, for: .navigation)
    }
    .accessibilityIdentifier("map.sidebar.content")
  }

  private var sidebarSearch: some View {
    VStack(spacing: 11) {
      MapSearchBar(
        query: $query, focused: $isSearchFocused, height: max(44, searchHeight),
        onSubmit: { store.search(query, debounce: false) },
        user: account.user, avatar: account.avatar,
        onAccount: {
          isSearchFocused = false
          modal = .account(.profile)
        },
        onVoiceSearch: {
          switch VoiceSearchButtonState(isVoicePanelOpen: showsVoiceSearch, state: voice.state) {
          case .start: startVoiceSearch()
          case .stop: finishVoiceSearch()
          case .recognizing: break
          case .close: cancelVoiceSearch()
          }
        }, voiceActive: showsVoiceSearch, voiceState: voice.state, language: preferences.language,
        showsAccount: false
      ).padding(.horizontal, 11)
      if showsVoiceSearch {
        VoiceSearchControls(
          voice: voice, onFinish: finishVoiceSearch, onRetry: startVoiceSearch,
          onKeyboard: {
            cancelVoiceSearch()
            isSearchFocused = true
          }, onCancel: cancelVoiceSearch
        )
        .padding(.horizontal, 11)
      }
      if store.browse != nil || store.pendingNearby != nil {
        ScrollView {
          PlaceResultsView(store: store, onSelect: selectPlace) {
            cancelVoiceSearch()
            query = ""
            store.closeResults()
          }.padding(.horizontal, 11).padding(.bottom, 12)
        }
      } else {
        MapPanelContent(
          preferences: preferences, onSettings: { modal = .settings($0) },
          cardHeight: cardHeight, showsSettings: false, bottomInset: 12,
          viewportState: store.viewportState, onRetry: store.retryResults,
          bookmarks: [], onCategory: showNearby, onSelect: selectPlace,
          onUnavailableAction: { showsUnavailableAction = true }, singleColumnCategories: true)
      }
    }
    .scrollDismissesKeyboard(.interactively)
  }

  @ViewBuilder private func placeDetails(bottomInset: CGFloat, reportsContentHeight: Bool)
    -> some View
  {
    if let selectedPlace {
      PlaceDetailView(
        place: selectedPlace, bottomInset: bottomInset,
        state: account.selectedMarker == nil ? store.detailState : account.detailState,
        onRetry: {
          if let marker = account.selectedMarker {
            account.select(marker)
          } else {
            store.retryDetail()
          }
        },
        onShare: {
          if store.isPreview {
            showsUnavailableAction = true
          } else {
            modal = .share(selectedPlace)
          }
        },
        onNavigate: { navigate(selectedPlace) },
        onEdit: { if let id = Int64(selectedPlace.id) { beginContribution(.edit(id)) } },
        isBookmarked: Int64(selectedPlace.id).map(account.isBookmarked) ?? false,
        bookmarkBusy: account.isBusy || account.bookmarkStatusLoading,
        onBookmark: { bookmark(selectedPlace) },
        authenticatedPhoto: account.selectedMarker != nil,
        photo: account.selectedPhoto, photoFailed: account.photoFailed,
        loadPrivatePhoto: account.placePhoto,
        reportsContentHeight: reportsContentHeight,
        onContentHeight: { height in
          // The panel narrows during a drag. Measure its resting full width,
          // without feeding transient wrapping back into the drag geometry.
          guard height > 0 else { return }
          measuredDetail = (selectedPlace.id, height)
        },
        onUnavailableAction: { showsUnavailableAction = true })
    }
  }

  private func panel(layout: PanelLayout, height: CGFloat) -> some View {
    VStack(spacing: 0) {
      grabberPlaceholder(layout: layout)
      if selectedPlace != nil {
        placeDetails(
          bottomInset: layout.bottomInset,
          reportsContentHeight: dragTranslation == 0 && contentDragTop == nil && detent != .collapsed)
      } else if sidebarDestination == .bookmarks && account.user != nil {
        NavigationStack {
          AccountPlacesView(store: account, created: false, onSelect: selectAccountPlace)
            .toolbar {
              ToolbarItem(placement: .topBarTrailing) {
                Button("Close") { sidebarDestination = .search }
                  .accessibilityIdentifier("map.bookmarks.close")
              }
            }
        }
      } else {
        MapSearchBar(
          query: $query, focused: $isSearchFocused, height: max(44, searchHeight),
          onSubmit: { store.search(query, debounce: false) },
          user: account.user, avatar: account.avatar,
          onAccount: {
            isSearchFocused = false
            if store.isPreview { showsUnavailableAction = true } else { modal = .account(.profile) }
          },
          onVoiceSearch: {
            switch VoiceSearchButtonState(
              isVoicePanelOpen: showsVoiceSearch, state: voice.state)
            {
            case .start:
              startVoiceSearch()
            case .stop:
              finishVoiceSearch()
            case .recognizing:
              break
            case .close:
              cancelVoiceSearch()
            }
          }, voiceActive: showsVoiceSearch, voiceState: voice.state,
          language: preferences.language
        )
        .padding(.horizontal, 14)
        .padding(.bottom, detent == .collapsed ? 14 : detent == .nearby ? 7 : 11)
        .contentShape(Rectangle())

        if showsVoiceSearch {
          VoiceSearchControls(
            voice: voice, onFinish: finishVoiceSearch, onRetry: startVoiceSearch,
            onKeyboard: {
              cancelVoiceSearch()
              isSearchFocused = true
            }, onCancel: cancelVoiceSearch
          )
          .padding(.horizontal, 14).padding(.bottom, 11)
        }

        Group {
          if store.browse != nil || store.pendingNearby != nil {
            ScrollView {
              PlaceResultsView(store: store, onSelect: selectPlace) {
                cancelVoiceSearch()
                query = ""
                store.closeResults()
              }
              .padding(.horizontal, 14)
              .padding(.bottom, keyboardHeight > 0 ? 12 : max(layout.bottomInset, 12))
            }
          } else {
            MapPanelContent(
              preferences: preferences,
              onSettings: {
                isSearchFocused = false
                modal = .settings($0)
              },
              cardHeight: cardHeight, showsSettings: detent == .expanded || contentDragTop != nil,
              bottomInset: keyboardHeight > 0 ? 12 : max(layout.bottomInset, 12),
              viewportState: store.viewportState, onRetry: store.retryResults,
              bookmarks: store.isPreview ? bookmarks : account.bookmarks.map(store.presentation),
              showsBookmarks: account.user != nil,
              bookmarksLoading: account.libraryLoading, bookmarksMessage: account.libraryMessage,
              onBookmarks: {
                if store.isPreview {
                  showsUnavailableAction = true
                } else {
                  modal = .account(.bookmarks)
                }
              },
              onCategory: showNearby, onSelect: selectPlace,
              onUnavailableAction: { showsUnavailableAction = true })
          }
        }
        .scrollIndicators(.hidden)
        .scrollDismissesKeyboard(.interactively)
        .scrollDisabled(detent == .collapsed)
        .accessibilityHidden(height <= layout.headerHeight + 1)
        .allowsHitTesting(height > layout.headerHeight + 1)
        .ignoresSafeArea(.container, edges: .bottom)
        .padding(.bottom, keyboardHeight)
      }
    }
    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    .contentShape(Rectangle())
    .gesture(PanelScrollGesture(
      layout: layout, restingTop: layout.top(for: detent),
      isExpanded: detent == .expanded,
      generation: panelGestureGeneration,
      onMove: { top in
        isSearchFocused = false
        contentDragTop = top
      },
      onEnd: { top, velocity in
        let target = layout.nearest(to: top + velocity * 0.18)
        withAnimation(reduceMotion ? nil : .spring(response: 0.36, dampingFraction: 0.86)) {
          contentDragTop = nil
          movePanel(to: target)
        }
      }))
    .onChange(of: detent) { _, _ in cancelPanelDrag() }
    .onDisappear { cancelPanelDrag() }
  }

  /// In-panel transparent placeholder that reserves the original 44→14pt layout
  /// space and draws nothing. It is not an accessibility element; the real handle
  /// is a sibling and draws the visible capsule.
  private func grabberPlaceholder(layout: PanelLayout) -> some View {
    let progress = layout.collapsedProgress(
      at: layout.clampedTop(contentDragTop ?? (layout.top(for: detent) + dragTranslation)))
    return Color.clear
      .frame(maxWidth: .infinity)
      .frame(height: 44 - 30 * progress)
      .accessibilityHidden(true)
      .allowsHitTesting(false)
  }

  /// The real 44pt handle Button. It lives outside the panel's clipShape as a
  /// root-ZStack sibling so it stays fully tappable while collapsed; its bottom
  /// edge aligns with the in-panel placeholder and it overhangs 30pt upward.
  private func grabberButton(layout: PanelLayout, panelTop: CGFloat) -> some View {
    let progress = layout.collapsedProgress(at: panelTop)
    return Button {
      movePanel(to: detent == .collapsed ? .nearby : detent == .nearby ? .expanded : .collapsed)
    } label: {
      Capsule().fill(.secondary.opacity(0.4))
        .frame(width: 48, height: 4)
        // The visible capsule keeps its exact old position within the 44pt
        // button: 15*progress below centre puts it at the placeholder midline.
        .offset(y: 15 * progress)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .contentShape(Rectangle())
    }
    .buttonStyle(.plain)
    .frame(width: layout.grabberWidth(at: panelTop), height: PanelLayout.grabberRealHeight)
    // Make the whole 44pt frame tappable, not only the 4pt capsule glyph.
    .contentShape(Rectangle())
    .accessibilityLabel("Map panel")
    .accessibilityValue(
      selectedPlace != nil && detent == .nearby ? Text("Place details") : detent.accessibilityName
    )
    .accessibilityIdentifier("map.panel.handle")
    .accessibilitySortPriority(2)
    .accessibilityAdjustableAction { direction in
      let states = MapPanelDetent.allCases
      guard let index = states.firstIndex(of: detent) else { return }
      switch direction {
      case .increment: movePanel(to: states[min(index + 1, states.count - 1)])
      case .decrement: movePanel(to: states[max(index - 1, 0)])
      @unknown default: break
      }
    }
    .highPriorityGesture(panelDrag(layout: layout))
  }

  private func panelDrag(layout: PanelLayout) -> some Gesture {
    DragGesture(minimumDistance: 10, coordinateSpace: .global)
      .updating($dragTranslation) { value, translation, _ in
        if abs(value.translation.height) > abs(value.translation.width) {
          translation = value.translation.height
        }
      }
      .onChanged { value in
        guard abs(value.translation.height) > abs(value.translation.width) else { return }
        isSearchFocused = false
      }
      .onEnded { value in
        guard abs(value.translation.height) > abs(value.translation.width) else { return }
        let target = layout.nearest(
          to: layout.top(for: detent) + value.predictedEndTranslation.height)
        movePanel(to: target)
      }
  }

  private func selectPlace(_ place: PlacePresentation) {
    sidebarContentVisible = true
    cancelVoiceSearch()
    if let marker = (account.bookmarks + account.created).first(where: { String($0.id) == place.id }
    ) {
      selectAccountPlace(marker)
      return
    }
    account.closeDetail()
    isSearchFocused = false
    withAnimation(reduceMotion ? nil : .spring(response: 0.36, dampingFraction: 0.86)) {
      store.select(place)
      detent = .nearby
    }
  }

  private func showNearby(_ category: PlaceCategory) {
    sidebarDestination = .search
    sidebarContentVisible = true
    guard store.isPreview || screenCenter != nil else {
      coordinateUnavailable()
      return
    }
    cancelVoiceSearch()
    account.closeDetail()
    query = ""
    let token = store.nearby(category)
    movePanel(to: .nearby)
    guard !store.isPreview else { return }
    location.request { result in
      if case .success(let point) = result { store.resolveNearbyLocation(point, token: token) }
    }
  }

  private func locate() {
    mapCoordinates.resolveIfNeeded(retryPending: true)
    requestLocation(showFailure: true)
  }

  private func requestStartupLocation() {
    guard scenePhase == .active, !store.isPreview, !location.isRequesting else { return }
    guard !location.hasRequestedLocation || (awaitsLocationAuthorization && location.isAuthorized)
    else { return }
    awaitsLocationAuthorization = false
    requestLocation(showFailure: false)
  }

  private func requestLocation(showFailure: Bool) {
    guard !store.isPreview else {
      showsUnavailableAction = true
      return
    }
    let token = store.beginLocationRequest()
    location.refreshAuthorization()
    let followsImmediately = showFailure && location.isAuthorized
    if followsImmediately { store.followUserLocation(token: token) }
    location.request { result in
      if case .failure(.denied) = result { awaitsLocationAuthorization = true }
      guard store.acceptsLocation(token) else { return }
      if !showFailure && (modal != nil || selectingLocation || isSearchFocused) { return }
      switch result {
      case .success(let point):
        // A manual tap has already started native following. A later Core Location
        // fix updates canonical data without pulling back a map the user has panned.
        store.locate(point, token: token, focusMap: !followsImmediately)
      case .failure(let failure):
        guard showFailure else { return }
        locationDenied = failure == .denied
        showsLocationError = true
      }
    }
  }

  private func navigate(_ place: PlacePresentation) {
    guard !store.isPreview else {
      showsUnavailableAction = true
      return
    }
    guard let point = place.point else { return }
    guard let coordinate = mapCoordinates.space.coordinate(for: point) else {
      coordinateUnavailable()
      return
    }
    let item = MKMapItem(
      location: CLLocation(latitude: coordinate.latitude, longitude: coordinate.longitude),
      address: nil)
    item.name = place.title
    if !item.openInMaps(launchOptions: [
      MKLaunchOptionsDirectionsModeKey: MKLaunchOptionsDirectionsModeWalking
    ]) {
      showsNavigationError = true
    }
  }

  private func movePanel(to newDetent: MapPanelDetent) {
    cancelPanelDrag()
    if newDetent != .expanded {
      cancelVoiceSearch()
      isSearchFocused = false
    }
    withAnimation(reduceMotion ? nil : .spring(response: 0.36, dampingFraction: 0.86)) {
      if newDetent == .collapsed {
        sidebarDestination = .search
        store.closeDetail()
        account.closeDetail()
      }
      detent = newDetent
    }
  }

  private func cancelPanelDrag() {
    contentDragTop = nil
    panelGestureGeneration &+= 1
  }

  private func selectAccountPlace(_ marker: Marker) {
    sidebarContentVisible = true
    cancelVoiceSearch()
    isSearchFocused = false
    account.select(marker)
    store.focusAccountPlace(store.presentation(marker))
    movePanel(to: .nearby)
  }

  private func bookmark(_ place: PlacePresentation) {
    guard !store.isPreview else {
      showsUnavailableAction = true
      return
    }
    guard let id = Int64(place.id) else { return }
    guard account.user != nil else {
      bookmarkIntent = UUID()
      pendingBookmark = id
      modal = .account(.profile)
      return
    }
    Task {
      await account.toggleBookmark(id, marker: account.selectedMarker ?? store.selectedMarker)
      showsAccountError = account.message != nil
    }
  }

  private func resumeBookmark() {
    guard let id = pendingBookmark, let owner = account.user?.publicId else { return }
    let token = account.epoch
    let intent = bookmarkIntent
    pendingBookmark = nil
    Task {
      await account.loadLibrary()
      guard owner == account.user?.publicId, token == account.epoch, intent == bookmarkIntent else {
        return
      }
      if !account.isBookmarked(id) { await account.toggleBookmark(id) }
      guard owner == account.user?.publicId, token == account.epoch, intent == bookmarkIntent else {
        return
      }
      modal = nil
      showsAccountError = account.message != nil
    }
  }

  private func resumeAuthenticatedAction() {
    contribution.synchronize()
    if let intent = contributionIntent {
      queuedContribution = intent
      modal = nil
    } else {
      resumeBookmark()
    }
  }

  private func beginContribution(_ intent: ContributionIntent) {
    cancelVoiceSearch()
    guard !store.isPreview else {
      showsUnavailableAction = true
      return
    }
    guard account.user != nil else {
      contributionIntent = intent
      modal = .account(.profile)
      return
    }
    contribution.synchronize()
    isSearchFocused = false
    if let draft = contribution.draft, draft.phase != .complete {
      modal = .contribution()
      return
    }
    switch intent {
    case .create:
      enterLocationSelection()
    case .edit(let id):
      modal = .contribution(editID: id)
    }
  }

  private func enterLocationSelection(at point: GeoPoint? = nil) {
    guard account.user != nil else { return }
    movePanel(to: .collapsed)
    pickedLocation = point
    if let point { store.focusMap(on: point) }
    selectingLocation = true
  }

  private func pickLocation(_ point: GeoPoint) {
    guard selectingLocation else { return }
    do {
      // A map tap is the selection; GPS only moves the camera.
      try contribution.begin(at: point)
      try contribution.move(to: point)
      locationPickFeedback += 1
      selectingLocation = false
      pickedLocation = nil
      modal = .contribution()
    } catch {
      contributionError = String(appLocalized: "Could not save the contribution on this device.")
    }
  }

  private func coordinateUnavailable() {
    mapCoordinates.resolveIfNeeded(retryPending: true)
    showsCoordinateError = true
  }

  private func startVoiceSearch() {
    sidebarDestination = .search
    sidebarContentVisible = true
    cancelVoiceSearch()
    voice = VoiceSearchController()
    account.closeDetail()
    store.closeDetail()
    isSearchFocused = false
    query = ""
    showsVoiceSearch = true
    movePanel(to: .expanded)
    let controller = voice
    let language = preferences.language
    voiceTask = Task {
      guard !Task.isCancelled else { return }
      await controller.start(language: language)
    }
  }

  private func finishVoiceSearch() {
    guard showsVoiceSearch else { return }
    if voice.state == .recording {
      voice.finish()
      return
    }
    if voice.state == .finishing { return }
    let text = voice.transcript.trimmingCharacters(in: .whitespacesAndNewlines)
    cancelVoiceSearch()
    query = text
    if !text.isEmpty { store.search(text, debounce: false) }
  }

  private func cancelVoiceSearch() {
    guard showsVoiceSearch || voiceTask != nil else { return }
    showsVoiceSearch = false
    voiceTask?.cancel()
    voiceTask = nil
    voice.stop()
  }
}

private enum ContributionIntent {
  case create
  case edit(Int64)
}

private enum MapModal: Identifiable {
  case account(AccountDestination)
  case share(PlacePresentation)
  case contribution(editID: Int64? = nil)
  case settings(SettingsDestination)
  case settingsHome
  case mapAppearance(GeoPoint?)
  case link(PlaceLink)
  var id: String {
    switch self {
    case .account: "account"
    case .share(let place): "share-\(place.id)"
    case .contribution(let id): "contribution-\(id.map(String.init) ?? "new")"
    case .settingsHome: "settings-home"
    case .settings(let destination): "settings-\(destination.rawValue)"
    case .mapAppearance: "map-appearance"
    case .link(let link): "link-\(link.id)"
    }
  }
}

extension MapPanelDetent {
  fileprivate var accessibilityName: Text {
    switch self {
    case .collapsed: Text("Collapsed")
    case .nearby: Text("Nearby")
    case .expanded: Text("Expanded")
    }
  }
}
