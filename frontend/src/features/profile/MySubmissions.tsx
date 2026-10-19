import { useQuery } from '@tanstack/react-query'
import { useSession } from '@/features/auth/SessionProvider'
import { useUi } from '@/shared/i18n/ui'
import { readMySubmissions, type Submission } from '@/shared/api/mySubmissions'
import { privateKeys } from '@/shared/query/keys'
import './mySubmissions.css'
import { localizedReason } from '@/features/admin/reason'

/** Profile-scoped translations; the shared table has no such keys yet. */
export const submissionMessages: Record<string, string> = {
    'My submissions': '我的提交',
    'New place': '新增点位',
    'Content changes': '内容修改',
    'Photo submission': '图片',
    Approved: '已通过',
    'Not approved': '未通过',
    'Pending review': '待审核',
    'Information is inaccurate': '信息不准确',
    'Duplicate place': '重复点位',
    'Wrong location': '位置错误',
    'Inappropriate photo': '照片不合适',
    'Unrelated to accessibility or friendly facilities': '与无障碍/友善设施无关',
    'Proposal is out of date; ask the contributor to resubmit from the latest content':
        '提案已过期，请按最新内容重新提交',
}

const kindLabel: Record<Submission['kind'], string> = {
    marker: 'New place',
    edit: 'Content changes',
    image: 'Photo submission',
}

const statusLabel: Record<Submission['status'], string> = {
    PENDING: 'Pending review',
    APPROVED: 'Approved',
    REJECTED: 'Not approved',
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
        ui.language === 'zh' && Object.hasOwn(submissionMessages, key)
            ? submissionMessages[key]!
            : ui.message(key)
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
                                {t(statusLabel[submission.status])}
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
                                {submission.reviewNote
                                    ? `: ${localizedReason(submission.reviewNote, t)}`
                                    : ''}
                            </p>
                        )}
                    </li>
                ))}
            </ul>
        </section>
    )
}
