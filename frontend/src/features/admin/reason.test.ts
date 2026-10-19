import { localizedReason } from './reason'

const zh: Record<string, string> = {
    'Information is inaccurate': '信息不准确',
    'Duplicate place': '重复点位',
    'Proposal is out of date; ask the contributor to resubmit from the latest content':
        '提案已过期，请按最新内容重新提交',
}
const t = (key: string) => zh[key] ?? key

it('translates known shortcut segments and keeps free text verbatim', () => {
    expect(localizedReason('Information is inaccurate · typo in the name', t)).toBe(
        '信息不准确 · typo in the name',
    )
    expect(localizedReason('Duplicate place', t)).toBe('重复点位')
})

it('translates the stale-proposal shortcut too', () => {
    expect(
        localizedReason(
            'Proposal is out of date; ask the contributor to resubmit from the latest content',
            t,
        ),
    ).toBe('提案已过期，请按最新内容重新提交')
})
