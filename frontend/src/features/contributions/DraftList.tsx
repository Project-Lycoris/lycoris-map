import { useInRouterContext } from 'react-router'
import { useState } from 'react'
import { useContributions } from './ContributionsProvider'
import { contributionBusy } from './ContributionStore'
import { useAccountFlow } from '@/features/auth/AccountFlow'
import { usePanelRoute } from '@/layouts/usePanelRoute'
import { useUi } from '@/shared/i18n/ui'
import { DesignButton } from '@/shared/ui/design-primitives'

export function DraftList() {
    return useInRouterContext() ? <RoutedDraftList /> : null
}
function RoutedDraftList() {
    const { store, state } = useContributions(),
        flow = useAccountFlow(),
        ui = useUi()
    const { open } = usePanelRoute(false, 'dismiss', true)
    const [working, setWorking] = useState(false),
        [error, setError] = useState<string | null>(null)
    if (!store || !state) return null
    const run = async (action: () => Promise<unknown>) => {
        if (working) return
        setWorking(true)
        setError(null)
        try {
            await action()
        } catch {
            setError('Could not save the draft on this device.')
        } finally {
            setWorking(false)
        }
    }
    const busy = working || contributionBusy(state.phase)
    return (
        <section className="draft-list" aria-label={ui.text('Drafts')}>
            <h3>{ui.text('Drafts')}</h3>
            <p>{ui.message('Drafts are saved on this device only.')}</p>
            {state.drafts.map((draft) => (
                <div className="draft-row" key={draft.id}>
                    <span>
                        {draft.title || ui.text('Untitled place')}
                        <time dateTime={new Date(draft.updatedAt).toISOString()}>
                            {new Date(draft.updatedAt).toLocaleString(
                                ui.language === 'zh' ? 'zh-CN' : 'en',
                                { dateStyle: 'short', timeStyle: 'short' },
                            )}
                        </time>
                    </span>
                    <DesignButton
                        disabled={busy}
                        onClick={() =>
                            void run(async () => {
                                if (await store.resumeDraft(draft.id)) {
                                    flow?.close()
                                    open('contribute-form', '', false, undefined, { snap: 'full' })
                                }
                            })
                        }
                    >
                        {ui.text('Continue')}
                    </DesignButton>
                    <DesignButton
                        disabled={busy}
                        aria-label={`${ui.text('Delete draft')}: ${draft.title || ui.text('Untitled place')}`}
                        onClick={() => void run(() => store.deleteDraft(draft.id))}
                    >
                        {ui.text('Delete draft')}
                    </DesignButton>
                </div>
            ))}
            <DesignButton
                disabled={busy}
                onClick={() =>
                    void run(async () => {
                        if (await store.newDraft()) {
                            flow?.close()
                            open('contribute', '', false, undefined, { snap: 'half' })
                        }
                    })
                }
            >
                {ui.text('New draft')}
            </DesignButton>
            {(error || state.persistenceError) && (
                <p role="alert">{ui.message(error || state.persistenceError)}</p>
            )}
        </section>
    )
}
