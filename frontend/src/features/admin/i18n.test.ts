import fs from 'node:fs'
import path from 'node:path'
import { adminMessages } from './ui'
import { uiMessages } from '@/shared/i18n/ui'
import { submissionMessages } from '@/features/profile/MySubmissions'

const root = process.cwd()
const adminDir = path.join(root, 'src/features/admin')
const files = [
    ...fs
        .readdirSync(adminDir)
        .filter((name) => name.endsWith('.tsx') && !name.endsWith('.test.tsx'))
        .map((name) => path.join(adminDir, name)),
    path.join(root, 'src/features/profile/MySubmissions.tsx'),
]

function usedKeys(file: string) {
    const source = fs.readFileSync(file, 'utf8')
    const keys = new Set<string>()
    const pattern = /(?:ui\.message|(?<![A-Za-z0-9_.])t)\(\s*'([^']+)'/g
    for (const match of source.matchAll(pattern)) keys.add(match[1]!)
    return keys
}

it('has a Chinese translation for every literal ui.message/t key used by admin and MySubmissions', () => {
    const known = new Set([
        ...Object.keys(adminMessages),
        ...Object.keys(uiMessages),
        ...Object.keys(submissionMessages),
    ])
    const missing = [...new Set(files.flatMap((file) => [...usedKeys(file)]))]
        .filter((key) => !known.has(key))
        .sort()
    expect(missing).toEqual([])
})

it('exposes the visible status and stale labels in Chinese', () => {
    for (const key of [
        'Sort',
        'Oldest first',
        'Newest first',
        'Search title, submitter or place ID',
        'Out of date',
        'Proposal is out of date; ask the contributor to resubmit from the latest content',
        'Account',
        'Skipped',
    ])
        expect(adminMessages[key as keyof typeof adminMessages]).toBeTruthy()
    expect(submissionMessages['Pending review']).toBe('待审核')
    expect(submissionMessages['Not approved']).toBe('未通过')
})
