import type { ContributionSnapshot } from './ContributionStore'
import type { MarkerCreate, MarkerText } from '@/shared/api/markerWrites'
export type SavedDraft = {
    id: string
    owner: string
    updatedAt: number
    snapshot: ContributionSnapshot
    frozen: MarkerCreate | MarkerText | null
    photoIds: string[]
    uploadedCount: number
}
export type DraftSummary = { id: string; title: string; updatedAt: number }
export interface DraftJournal {
    list(owner: string): Promise<SavedDraft[]>
    put(value: SavedDraft): Promise<void>
    remove(id: string, owner: string): Promise<void>
}
/** IndexedDB stores Files and the exact retry receipt together in one transaction. */
export class BrowserDraftJournal implements DraftJournal {
    private async database(): Promise<IDBDatabase> {
        return new Promise((resolve, reject) => {
            const request = indexedDB.open('lycoris-contributions', 1)
            request.onupgradeneeded = () =>
                request.result.createObjectStore('drafts', { keyPath: 'id' })
            request.onerror = () => reject(request.error)
            request.onsuccess = () => resolve(request.result)
        })
    }
    async list(owner: string) {
        const db = await this.database()
        try {
            const rows = await new Promise<SavedDraft[]>((resolve, reject) => {
                const request = db.transaction('drafts').objectStore('drafts').getAll()
                request.onsuccess = () => resolve(request.result as SavedDraft[])
                request.onerror = () => reject(request.error)
            })
            return rows
                .filter((row) => row.owner === owner)
                .sort((a, b) => b.updatedAt - a.updatedAt)
        } finally {
            db.close()
        }
    }
    async put(value: SavedDraft) {
        await this.write((store) => {
            store.put(value)
        })
    }
    async remove(id: string, owner: string) {
        await this.write((store) => {
            const get = store.get(id)
            get.onsuccess = () => {
                if (get.result?.owner === owner) store.delete(id)
            }
        })
    }
    private async write(action: (store: IDBObjectStore) => void) {
        const db = await this.database()
        try {
            await new Promise<void>((resolve, reject) => {
                const transaction = db.transaction('drafts', 'readwrite')
                transaction.oncomplete = () => resolve()
                transaction.onerror = () => reject(transaction.error)
                transaction.onabort = () => reject(transaction.error)
                action(transaction.objectStore('drafts'))
            })
        } finally {
            db.close()
        }
    }
}
