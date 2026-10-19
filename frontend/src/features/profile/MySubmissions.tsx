import { useQuery } from '@tanstack/react-query'
import { useSession } from '@/features/auth/SessionProvider'
import { useUi } from '@/shared/i18n/ui'
import { readMySubmissions, type Submission } from '@/shared/api/mySubmissions'
import { privateKeys } from '@/shared/query/keys'
import './mySubmissions.css'

/** Profile-scoped translations; the shared table has no such keys yet. */
const messages: Record<string, string> = {
    'My submissions': '我的提交',
    'New place': '新增点位',
    'Content changes': '内容修改',
    'Photo submission': '图片',
    'Not approved': '未通过',
    pending: '待审核',
    approved: '已通过',
    rejected: '已拒绝',
}

const kindLabel: Record<Submission['kind'], string> = {
    marker: 'New place',
    edit: 'Content changes',
    image: 'Photo submission',
}

export function MySubmissions() {
    const ui = useUi(),
        { store, scope, busy } = useSession()
    const query = useQuery({
        queryKey: scope
            ? [...privateKeys.created(scope, ui.language), 'submissions']
            : ['private', 'submissions-idle'],
        enabled: !!scope && !busy,
        gcTime: 0,
        retry: false,
        queryFn: ({ signal }) =>
            store!.runPrivate(scope!, (s) => readMySubmissions(ui.language, s), signal),
    })
    const t = (key: string) =>
        ui.language === 'zh' && Object.hasOwn(messages, key) ? messages[key]! : ui.message(key)
    // A missing route (404) resolves to null and an unexpected failure must not
    // take down the rest of the profile.
    if (query.isError || !query.data?.length) return null
    return (
        <section className="my-submissions" aria-label={t('My submissions')}>
            <h3>{t('My submissions')}</h3>
            <ul role="list">
                {query.data.map((submission) => (
                    <li key={`${submission.kind}-${submission.id}`}>
                        <p className="submission-title">
                            <span className="submission-kind">{t(kindLabel[submission.kind])}</span>{' '}
                            {submission.markerTitle}
                        </p>
                        <p className="submission-meta">
                            <span
                                className={`submission-status submission-${submission.status.toLowerCase()}`}
                            >
                                {t(submission.status.toLowerCase())}
                            </span>
                            <time dateTime={submission.createdAt}>
                                {new Intl.DateTimeFormat(ui.language === 'zh' ? 'zh-CN' : 'en-US', {
                                    dateStyle: 'medium',
                                }).format(new Date(submission.createdAt))}
                            </time>
                        </p>
                        {submission.status === 'REJECTED' && (
                            <p className="submission-note">
                                {t('Not approved')}
                                {submission.reviewNote ? `: ${submission.reviewNote}` : ''}
                            </p>
                        )}
                    </li>
                ))}
            </ul>
        </section>
    )
}
