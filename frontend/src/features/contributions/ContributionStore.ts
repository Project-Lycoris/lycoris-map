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
import { draftPhotos } from './draft'
import type { DraftJournal, SavedDraft, DraftSummary } from './DraftJournal'

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
    drafts: DraftSummary[]
    persistenceError: string | null
    uploadedCount: number
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
        drafts: [],
        persistenceError: null,
        uploadedCount: 0,
    }
}
const uncertain = (error: unknown) =>
    !(error instanceof ApiError) ||
    error.status === 0 ||
    error.status === 408 ||
    error.status >= 500
const errorMessage = (error: unknown) =>
    error instanceof Error ? error.message : 'Submission failed. Please try again.'

/** The persisted receipt makes text creation and individual photo retries stable. */
export class ContributionStore {
    private snapshot: ContributionSnapshot
    private readonly listeners = new Set<() => void>()
    private frozen: writes.MarkerCreate | writes.MarkerText | null = null
    private photoRound = 0
    private photoIds: string[] = []
    private saveQueue: Promise<void> = Promise.resolve()
    private libraryRound = 0
    private uploadController: AbortController | null = null
    constructor(
        private readonly client: QueryClient,
        private readonly session: Session,
        private readonly api: ContributionApi = writes,
        private readonly validatePhoto: (file: File) => Promise<void> = inspectPhoto,
        private readonly upload: typeof resumePhoto = resumePhoto,
        private readonly journal?: DraftJournal,
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
        this.persist()
    }
    clearStale = () => {
        if (this.snapshot.owner && !this.session.isCurrent(this.snapshot.owner)) {
            this.persist()
            this.reset(this.snapshot.language)
        } else if (!this.snapshot.owner && this.session.getSnapshot().scope)
            this.publish({ owner: this.session.getSnapshot().scope })
        void this.refreshDrafts()
    }
    private reset(language: Language) {
        this.uploadController?.abort()
        this.uploadController = null
        this.photoIds = []
        this.frozen = null
        this.photoRound++
        const drafts =
            this.snapshot.owner?.publicId === this.session.getSnapshot().scope?.publicId
                ? this.snapshot.drafts
                : []
        this.libraryRound++
        this.snapshot = fresh(this.snapshot.round + 1, language, this.session.getSnapshot().scope)
        this.snapshot.drafts = drafts
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
        this.persist()
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
            this.publish({
                draft: {
                    ...draft,
                    photo: this.snapshot.draft.photo,
                    photos: this.snapshot.draft.photos ?? [],
                },
                error: null,
            })
    }
    setPoint = (point: LatLng) => {
        if (this.snapshot.phase === 'draft' && !this.snapshot.base)
            this.publish({ point: { ...point }, error: null })
    }
    photo = async (file: File | null) => {
        if (file) await this.setPhotos([file])
        else await this.setPhotos([])
    }
    addPhotos = async (files: File[]) =>
        this.setPhotos([...draftPhotos(this.snapshot.draft), ...files])
    removePhoto = async (index: number) => {
        if (index < this.snapshot.uploadedCount) return
        await this.setPhotos(draftPhotos(this.snapshot.draft).filter((_, i) => i !== index))
    }
    private setPhotos = async (files: File[]) => {
        const previous = this.snapshot.phase
        if (!['draft', 'photo-error'].includes(previous)) return
        const round = this.snapshot.round,
            token = ++this.photoRound
        const old = draftPhotos(this.snapshot.draft),
            ids = this.photoIds
        this.publish({ phase: 'checking-photo', error: null })
        try {
            for (const file of files) if (!old.includes(file)) await this.validatePhoto(file)
            if (round !== this.snapshot.round || token !== this.photoRound) return
            this.photoIds = files.map((file) => ids[old.indexOf(file)] ?? crypto.randomUUID())
            this.publish({
                draft: { ...this.snapshot.draft, photo: files[0] ?? null, photos: files },
                phase:
                    this.snapshot.saved && files.length === this.snapshot.uploadedCount
                        ? 'complete'
                        : previous,
            })
        } catch (error) {
            if (round === this.snapshot.round && token === this.photoRound)
                this.publish({ phase: previous, error: errorMessage(error) })
        }
    }
    report = (message: string) => this.publish({ error: message })
    dispose = () => {
        this.persist()
        this.uploadController?.abort()
        this.photoRound++
        this.snapshot = { ...this.snapshot, round: this.snapshot.round + 1 }
    }
    private current(round: number, scope: PrivateScope) {
        return round === this.snapshot.round && this.session.isCurrent(scope)
    }
    private persist() {
        const value = this.snapshot,
            owner = value.owner?.publicId
        if (!this.journal || !owner) return
        if (!value.point && !value.draft.title && !draftPhotos(value.draft).length) return
        const row: SavedDraft = {
            id: value.requestId,
            owner,
            updatedAt: Date.now(),
            snapshot: { ...value, drafts: [] },
            frozen: this.frozen,
            photoIds: [...this.photoIds],
            uploadedCount: value.uploadedCount,
        }
        const write = async () => {
            if (value.phase === 'complete') await this.journal!.remove(row.id, owner)
            else await this.journal!.put(row)
        }
        this.saveQueue = this.saveQueue.catch(() => undefined).then(write)
        void this.saveQueue
            .then(() => {
                if (this.snapshot.owner?.publicId === owner) void this.refreshDrafts()
            })
            .catch(() => {
                if (this.snapshot.owner?.publicId === owner) {
                    this.snapshot = {
                        ...this.snapshot,
                        persistenceError: 'Could not save the draft on this device.',
                    }
                    this.listeners.forEach((fn) => fn())
                }
            })
    }
    private async checkpoint() {
        this.persist()
        await this.saveQueue
    }
    refreshDrafts = async () => {
        const owner = this.session.getSnapshot().scope?.publicId,
            round = ++this.libraryRound
        if (!this.journal) return
        try {
            const rows = owner ? await this.journal.list(owner) : []
            if (round !== this.libraryRound || owner !== this.session.getSnapshot().scope?.publicId)
                return
            this.snapshot = {
                ...this.snapshot,
                drafts: rows.map((row) => ({
                    id: row.id,
                    title: row.snapshot.draft.title,
                    updatedAt: row.updatedAt,
                })),
            }
            this.listeners.forEach((fn) => fn())
        } catch {
            if (
                round === this.libraryRound &&
                owner === this.session.getSnapshot().scope?.publicId
            ) {
                this.snapshot = {
                    ...this.snapshot,
                    persistenceError: 'Could not save the draft on this device.',
                }
                this.listeners.forEach((fn) => fn())
            }
        }
    }
    resumeDraft = async (id: string) => {
        const scope = this.session.getSnapshot().scope
        if (!this.journal || !scope || contributionBusy(this.snapshot.phase)) return false
        await this.checkpoint()
        const rows = await this.journal.list(scope.publicId),
            row = rows.find((row) => row.id === id)
        if (!row || !this.session.isCurrent(scope)) return false
        const drafts = this.snapshot.drafts,
            round = this.snapshot.round + 1
        this.uploadController?.abort()
        this.frozen = row.frozen
        this.photoIds = row.photoIds
        const phase =
            row.snapshot.phase === 'saving'
                ? 'save-uncertain'
                : row.snapshot.phase === 'photo-saving'
                  ? 'photo-paused'
                  : row.snapshot.phase === 'checking-photo'
                    ? 'draft'
                    : row.snapshot.phase
        this.snapshot = {
            ...row.snapshot,
            round,
            owner: scope,
            drafts,
            phase,
            uploadedCount: row.uploadedCount,
            persistenceError: null,
        }
        this.listeners.forEach((fn) => fn())
        return true
    }
    deleteDraft = async (id: string) => {
        const scope = this.session.getSnapshot().scope
        if (!this.journal || !scope || contributionBusy(this.snapshot.phase)) return
        await this.saveQueue.catch(() => undefined)
        await this.journal.remove(id, scope.publicId)
        if (!this.session.isCurrent(scope)) return
        if (this.snapshot.requestId === id) this.reset('zh')
        await this.refreshDrafts()
    }
    newDraft = async () => {
        if (contributionBusy(this.snapshot.phase)) return false
        await this.checkpoint()
        this.reset('zh')
        return true
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
                if (textUnchanged && !draftPhotos(initial.draft).length) {
                    this.frozen = null
                    this.publish({ phase: 'draft', error: 'There are no changes to submit.' })
                    return
                }
                await this.checkpoint()
                if (!this.current(round, scope)) return
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
        const photos = draftPhotos(draft)
        for (let index = this.snapshot.uploadedCount; index < photos.length; index++) {
            this.photoIds[index] ??= crypto.randomUUID()
            this.publish({ phase: 'photo-saving' })
            this.uploadController = new AbortController()
            try {
                await this.checkpoint()
                if (!this.current(round, scope)) return
                await this.upload(
                    saved!.id,
                    photos[index]!,
                    this.photoIds[index]!,
                    (task, signal) => this.session.runPrivate(scope, task, signal),
                    this.uploadController.signal,
                )
                if (!this.current(round, scope)) return
                this.publish({ uploadedCount: index + 1 })
            } catch (error) {
                if (!this.current(round, scope)) return
                this.publish({
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
