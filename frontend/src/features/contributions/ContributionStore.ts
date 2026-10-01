import type { QueryClient } from '@tanstack/react-query'
import type { SessionStore } from '@/features/auth/SessionStore'
import type { LatLng } from '@/features/map/coords'
import type { Marker } from '@/shared/api/markers'
import * as writes from '@/shared/api/markerWrites'
import { ApiError } from '@/shared/api/ApiError'
import { privateKeys, type Language, type PrivateScope } from '@/shared/query/keys'
import {
    checkPoint,
    draftFromMarker,
    draftText,
    emptyContributionDraft,
    type ContributionDraft,
} from './draft'
import { inspectPhoto } from './photo'
import { resumePhoto } from './resumePhoto'

export type ContributionPhase =
    | 'draft'
    | 'checking-photo'
    | 'saving'
    | 'save-uncertain'
    | 'photo-saving'
    | 'photo-error'
    | 'photo-paused'
    | 'complete'
export type ContributionSnapshot = {
    round: number
    draft: ContributionDraft
    point: LatLng | null
    base: Marker | null
    language: Language
    owner: PrivateScope | null
    requestId: string
    phase: ContributionPhase
    error: string | null
    saved: Marker | null
}
type Session = Pick<SessionStore, 'getSnapshot' | 'isCurrent' | 'runPrivate'>
export type ContributionApi = Pick<typeof writes, 'createMarker' | 'proposeMarkerEdit'>
const busyPhases: readonly ContributionPhase[] = ['checking-photo', 'saving', 'photo-saving']
export const contributionBusy = (phase: ContributionPhase) => busyPhases.includes(phase)
function fresh(
    round: number,
    language: Language,
    owner: PrivateScope | null,
): ContributionSnapshot {
    return {
        round,
        language,
        owner,
        requestId: crypto.randomUUID(),
        draft: { ...emptyContributionDraft },
        point: null,
        base: null,
        phase: 'draft',
        error: null,
        saved: null,
    }
}
const uncertain = (error: unknown) =>
    !(error instanceof ApiError) ||
    error.status === 0 ||
    error.status === 408 ||
    error.status >= 500
const errorMessage = (error: unknown) =>
    error instanceof Error ? error.message : 'Submission failed. Please try again.'

/** In-memory submission receipt: an image failure never repeats the marker write. */
export class ContributionStore {
    private snapshot: ContributionSnapshot
    private readonly listeners = new Set<() => void>()
    private frozen: writes.MarkerCreate | writes.MarkerText | null = null
    private photoRound = 0
    private uploadRequestId: string | null = null
    private uploadController: AbortController | null = null
    constructor(
        private readonly client: QueryClient,
        private readonly session: Session,
        private readonly api: ContributionApi = writes,
        private readonly validatePhoto: (file: File) => Promise<void> = inspectPhoto,
        private readonly upload: typeof resumePhoto = resumePhoto,
    ) {
        this.snapshot = fresh(0, 'zh', session.getSnapshot().scope)
    }
    subscribe = (fn: () => void) => {
        this.listeners.add(fn)
        return () => {
            this.listeners.delete(fn)
        }
    }
    getSnapshot = () => this.snapshot
    private publish(next: Partial<ContributionSnapshot>) {
        this.snapshot = { ...this.snapshot, ...next }
        this.listeners.forEach((fn) => fn())
    }
    clearStale = () => {
        if (this.snapshot.owner && !this.session.isCurrent(this.snapshot.owner))
            this.reset(this.snapshot.language)
        else if (!this.snapshot.owner && this.session.getSnapshot().scope)
            this.publish({ owner: this.session.getSnapshot().scope })
    }
    private reset(language: Language) {
        this.uploadController?.abort()
        this.uploadController = null
        this.uploadRequestId = null
        this.frozen = null
        this.photoRound++
        this.snapshot = fresh(this.snapshot.round + 1, language, this.session.getSnapshot().scope)
        this.listeners.forEach((fn) => fn())
    }
    beginCreate = (_language: Language) => {
        const language = 'zh'
        // Reopen the current work while it uploads; never replace an in-flight draft.
        if (contributionBusy(this.snapshot.phase)) return true
        if (this.snapshot.base || this.snapshot.phase === 'complete') this.reset(language)
        else if (!this.frozen) this.publish({ language })
        return true
    }
    beginEdit = (marker: Marker) => {
        if (contributionBusy(this.snapshot.phase)) return false
        if (this.snapshot.base?.id === marker.id && this.snapshot.phase !== 'complete') return true
        this.reset('zh')
        this.publish({
            base: marker,
            draft: draftFromMarker(marker),
            point: { lat: marker.lat, lng: marker.lng },
        })
        return true
    }
    change = (draft: ContributionDraft) => {
        if (this.snapshot.phase === 'draft')
            this.publish({ draft: { ...draft, photo: this.snapshot.draft.photo }, error: null })
    }
    setPoint = (point: LatLng) => {
        if (this.snapshot.phase === 'draft' && !this.snapshot.base)
            this.publish({ point: { ...point }, error: null })
    }
    photo = async (file: File | null) => {
        const previous = this.snapshot.phase
        if (!['draft', 'photo-error'].includes(previous)) return
        const round = this.snapshot.round,
            token = ++this.photoRound
        this.publish({ phase: 'checking-photo', error: null })
        try {
            if (file) await this.validatePhoto(file)
            if (round === this.snapshot.round && token === this.photoRound) {
                this.uploadRequestId = file ? crypto.randomUUID() : null
                this.publish({
                    draft: { ...this.snapshot.draft, photo: file },
                    phase:
                        previous === 'photo-error' && !file && this.snapshot.saved
                            ? 'complete'
                            : previous,
                })
            }
        } catch (error) {
            if (round === this.snapshot.round && token === this.photoRound)
                this.publish({ phase: previous, error: errorMessage(error) })
        }
    }
    report = (message: string) => this.publish({ error: message })
    dispose = () => this.reset(this.snapshot.language)
    private current(round: number, scope: PrivateScope) {
        return round === this.snapshot.round && this.session.isCurrent(scope)
    }
    submit = async (scope: PrivateScope, resendUnconfirmed = false) => {
        const initial = this.snapshot
        if (
            contributionBusy(initial.phase) ||
            initial.phase === 'complete' ||
            !this.session.isCurrent(scope)
        )
            return
        if (initial.owner && !this.session.isCurrent(initial.owner)) return
        if (initial.base && initial.phase === 'save-uncertain' && !resendUnconfirmed) return
        const round = initial.round
        this.publish({ owner: scope, error: null })
        if (!this.frozen) {
            try {
                const text = draftText(initial.draft, initial.language)
                if (initial.base) this.frozen = text
                else {
                    checkPoint(initial.point)
                    this.frozen = { ...text, ...initial.point, clientRequestId: initial.requestId }
                }
            } catch (error) {
                this.publish({ error: errorMessage(error) })
                return
            }
        }
        if (!initial.saved) {
            this.publish({ phase: 'saving' })
            try {
                const payload = this.frozen
                // A photo-only edit must not create a redundant text proposal.
                const textUnchanged =
                    initial.base &&
                    JSON.stringify(payload) ===
                        JSON.stringify(draftText(draftFromMarker(initial.base), initial.language))
                if (textUnchanged && !initial.draft.photo) {
                    this.frozen = null
                    this.publish({ phase: 'draft', error: 'There are no changes to submit.' })
                    return
                }
                const saved = textUnchanged
                    ? initial.base!
                    : await this.session.runPrivate(scope, (signal) =>
                          initial.base
                              ? this.api.proposeMarkerEdit(initial.base.id, payload, signal)
                              : this.api.createMarker(payload as writes.MarkerCreate, signal),
                      )
                if (!this.current(round, scope)) return
                this.publish({ saved })
                // Reads acquire the session lock too; invalidate only after runPrivate releases it.
                void this.client.invalidateQueries({ queryKey: privateKeys.scope(scope) })
            } catch (error) {
                if (!this.current(round, scope)) return
                if (uncertain(error) || initial.phase === 'save-uncertain')
                    this.publish({
                        phase: 'save-uncertain',
                        error: initial.base
                            ? 'The response was lost. Your edit may already be awaiting review. Sending again may create a duplicate proposal.'
                            : 'The result could not be confirmed. Retry this same submission to safely recover the place.',
                    })
                else {
                    this.frozen = null
                    this.publish({ phase: 'draft', error: errorMessage(error) })
                }
                return
            }
        }
        if (!this.current(round, scope)) return
        const { saved, draft } = this.snapshot
        if (draft.photo) {
            this.publish({ phase: 'photo-saving' })
            this.uploadRequestId ??= crypto.randomUUID()
            this.uploadController = new AbortController()
            try {
                await this.upload(
                    saved!.id,
                    draft.photo,
                    this.uploadRequestId,
                    (task, signal) => this.session.runPrivate(scope, task, signal),
                    this.uploadController.signal,
                )
                if (!this.current(round, scope)) return
            } catch (error) {
                if (!this.current(round, scope)) return
                this.publish({
                    // Only an explicit file rejection permits replacing it. An unknown
                    // receipt must keep the same photo UUID until reconciled.
                    phase:
                        error instanceof ApiError && [400, 410, 413, 415].includes(error.status)
                            ? 'photo-error'
                            : 'photo-paused',
                    error: `The place step is saved. Photo upload paused: ${errorMessage(error)}`,
                })
                return
            }
        }
        if (this.current(round, scope)) this.publish({ phase: 'complete', error: null })
    }
}
