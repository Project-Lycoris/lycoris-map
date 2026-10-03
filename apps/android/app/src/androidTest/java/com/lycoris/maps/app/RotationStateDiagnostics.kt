package com.lycoris.maps.app

import androidx.compose.runtime.State
import androidx.compose.runtime.snapshots.Snapshot
import com.lycoris.maps.core.device.HeadingState
import com.lycoris.maps.feature.map.PanelGeometry
import java.util.IdentityHashMap

/** Test-only, bounded observation of applied state; never changes sensors or app behavior. */
internal class RotationStateDiagnostics : AutoCloseable {
    private val lock = Any()
    private val headings = IdentityHashMap<Any, HeadingState>()
    private val geometries = IdentityHashMap<Any, PanelGeometry>()
    private var headingChanges = 0
    private var timestampOnlyChanges = 0
    private var geometryChanges = 0
    private var unobservableChanges = 0
    private var stateReadFailures = 0
    private var lastHeading: HeadingState? = null
    private var lastGeometry: PanelGeometry? = null
    private val observer = Snapshot.registerApplyObserver { changed, _ ->
        synchronized(lock) {
            for (state in changed) {
                if (state !is State<*>) {
                    unobservableChanges++
                    continue
                }
                val observed = runCatching { state.value }
                if (observed.isFailure) stateReadFailures++
                val value = observed.getOrNull()
                when (value) {
                    is HeadingState -> if (state in headings || headings.size < 32) {
                        val previous = headings.put(state, value)
                        if (previous != value) {
                            headingChanges++
                            if (previous != null && previous.copy(elapsedRealtimeNanos = value.elapsedRealtimeNanos) == value) {
                                timestampOnlyChanges++
                            }
                            lastHeading = value
                        }
                    }
                    is PanelGeometry -> if (state in geometries || geometries.size < 32) {
                        if (geometries.put(state, value) != value) geometryChanges++
                        lastGeometry = value
                    }
                }
            }
        }
    }

    fun summary(): String = synchronized(lock) {
        "headingChanges=$headingChanges,timestampOnlyChanges=$timestampOnlyChanges," +
            "headingObserved=${headings.isNotEmpty()},headingStates=${headings.size}," +
            "headingStatus=${lastHeading?.status},headingSource=${lastHeading?.source}," +
            "geometryChanges=$geometryChanges,geometryStates=${geometries.size},geometry=$lastGeometry," +
            "unobservableChanges=$unobservableChanges,stateReadFailures=$stateReadFailures"
    }

    override fun close() { observer.dispose() }
}
