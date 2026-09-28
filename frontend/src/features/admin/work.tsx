import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from 'react'
import { useQueryClient } from '@tanstack/react-query'
import { ApiError } from '@/shared/api/ApiError'
import { privateKeys, publicKeys } from '@/shared/query/keys'
import { DesignButton } from '@/shared/ui/design-primitives'
import { useAdminAccess } from './useAdminAccess'
import { useAdminUi } from './ui'
type Admin = ReturnType<typeof useAdminAccess>
type Confirmation = {
    label: string
    detail: string
    action: (signal: AbortSignal) => Promise<unknown>
}
type Work = Pick<Admin, 'run' | 'prefix' | 'session'> & {
    busy: boolean
    confirm: (value: Confirmation) => void
    clearConfirmation: () => void
    mutate: (action: Confirmation['action']) => Promise<boolean>
}
const Context = createContext<Work | null>(null)
export function useAdminWork() {
    const value = useContext(Context)
    if (!value) throw new Error('Admin work missing')
    return value
}
export function AdminWork({ admin, children }: { admin: Admin; children: ReactNode }) {
    const ui = useAdminUi(),
        client = useQueryClient(),
        [busy, setBusy] = useState(false),
        running = useRef(false)
    const [confirmation, setConfirmation] = useState<Confirmation | null>(null),
        [message, setMessage] = useState('')
    const trigger = useRef<HTMLElement | null>(null)
    const restore = () => {
        requestAnimationFrame(() => {
            if (trigger.current?.isConnected && !trigger.current.matches(':disabled'))
                trigger.current.focus()
            else document.getElementById('admin-title')?.focus()
        })
    }
    useEffect(() => {
        if (!confirmation) return
        const escape = (event: KeyboardEvent) => {
            if (event.key === 'Escape' && !event.isComposing) {
                event.preventDefault()
                setConfirmation(null)
                restore()
            }
        }
        window.addEventListener('keydown', escape)
        return () => window.removeEventListener('keydown', escape)
    }, [confirmation])
    const mutate: Work['mutate'] = async (action) => {
        if (running.current || admin.session.busy) return false
        const scope = admin.session.scope!,
            store = admin.session.store!
        running.current = true
        setBusy(true)
        setMessage('')
        setConfirmation(null)
        let success = false
        try {
            await admin.run(action)
            success = true
            if (store.isCurrent(scope)) setMessage('Completed.')
        } catch (error) {
            if (
                store.isCurrent(scope) &&
                !(error instanceof DOMException && error.name === 'AbortError')
            )
                setMessage(
                    error instanceof ApiError && error.status === 409
                        ? 'The item changed. Review the refreshed list.'
                        : !(error instanceof ApiError) || error.status === 0 || error.status >= 500
                          ? 'The outcome could not be confirmed. Refresh and inspect the item before trying again.'
                          : 'The request failed. Refresh and try again.',
                )
        } finally {
            try {
                if (store.isCurrent(scope)) {
                    // Refresh business data, not the dashboard's access gate. Each
                    // API request still checks the current role on the server.
                    // Never replay a write whose response may have been lost.
                    await Promise.all([
                        client.invalidateQueries({
                            queryKey: privateKeys.scope(scope),
                            predicate: (query) => query.queryKey.at(-1) !== 'access',
                        }),
                        client.invalidateQueries({ queryKey: publicKeys.markers() }),
                    ])
                }
            } catch {
                if (success && store.isCurrent(scope))
                    setMessage(
                        'Saved, but the list could not refresh. Refresh before reviewing another item.',
                    )
            } finally {
                running.current = false
                if (store.isCurrent(scope)) {
                    setBusy(false)
                    restore()
                }
            }
        }

        return success
    }
    return (
        <Context
            value={{
                ...admin,
                busy,
                clearConfirmation: () => setConfirmation(null),
                confirm: (value) => {
                    trigger.current =
                        document.activeElement instanceof HTMLElement
                            ? document.activeElement
                            : null
                    setConfirmation(value)
                },
                mutate,
            }}
        >
            {confirmation && (
                <section
                    className="admin-confirm"
                    role="alertdialog"
                    aria-modal="false"
                    aria-labelledby="admin-confirm-title"
                    aria-describedby="admin-confirm-detail"
                >
                    <h2 id="admin-confirm-title">{confirmation.label}</h2>
                    <p id="admin-confirm-detail">{confirmation.detail}</p>
                    <div className="admin-actions">
                        <DesignButton onClick={() => void mutate(confirmation.action)}>
                            {ui.message('Confirm')}
                        </DesignButton>
                        <DesignButton
                            autoFocus
                            onClick={() => {
                                setConfirmation(null)
                                restore()
                            }}
                        >
                            {ui.message('Cancel')}
                        </DesignButton>
                    </div>
                </section>
            )}
            {message && (
                <p role="status" className="admin-message">
                    {ui.message(message)}
                </p>
            )}
            {children}
        </Context>
    )
}
