import { DesignButton } from '@/shared/ui/design-primitives'
import { useAdminUi } from './ui'

export function Pagination({
    page,
    total,
    busy,
    change,
}: {
    page: number
    total: number
    busy: boolean
    change: (page: number) => void
}) {
    const ui = useAdminUi()
    return (
        <nav className="admin-pagination" aria-label={ui.message('Pages')}>
            <DesignButton disabled={busy || page === 0} onClick={() => change(page - 1)}>
                {ui.message('Previous')}
            </DesignButton>
            <span>
                {page + 1} / {total}
            </span>
            <DesignButton disabled={busy || page + 1 >= total} onClick={() => change(page + 1)}>
                {ui.message('Next')}
            </DesignButton>
        </nav>
    )
}
