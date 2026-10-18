import { describe, expect, it } from 'vitest'
import { localize } from './ui'

// B2-1: the photo-paused prefix branch used to hand the already-prefixed
// template back to `localize()`, which re-entered the branch forever and
// overflowed the stack, crashing the whole page (and draft recovery).
describe('localize photo-upload-paused prefix', () => {
    const paused = 'The place step is saved. Photo upload paused: '

    it('returns the English sentence unchanged', () => {
        const input = `${paused}Network error.`
        expect(localize('en', input)).toBe(input)
    })

    it('translates the sentence and keeps an untranslated error verbatim', () => {
        expect(localize('zh', `${paused}Network error.`)).toBe(
            '点位已保存，照片上传暂停：Network error.',
        )
    })

    it('translates an error that is itself a known message', () => {
        expect(localize('zh', `${paused}This place is unavailable.`)).toBe(
            '点位已保存，照片上传暂停：此点位暂不可用。',
        )
    })

    it('handles an empty error without throwing', () => {
        expect(localize('en', paused)).toBe(paused)
        expect(localize('zh', paused)).toBe('点位已保存，照片上传暂停：')
    })

    it('still fills ordinary templates', () => {
        expect(localize('zh', 'Version {version}', { version: '1.1.0' })).toBe('版本 1.1.0')
        expect(localize('en', 'Version {version}', { version: '1.1.0' })).toBe('Version 1.1.0')
    })
})

describe('localize sign-in rate limit', () => {
    it('translates the backend 42933 message', () => {
        expect(localize('zh', 'Too many sign-in attempts. Try again later.')).toBe(
            '登录尝试次数过多，请稍后再试。',
        )
    })
})
