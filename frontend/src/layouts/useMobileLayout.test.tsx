import { createElement } from 'react'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { MemoryRouter } from 'react-router'
import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import { MapShell } from './MapShell'
import { resetViewportStore } from './viewport'

type FakeVisualViewport = {
    height: number
    offsetTop: number
    scale: number
    addEventListener: (type: string, listener: () => void) => void
    removeEventListener: (type: string, listener: () => void) => void
    dispatch: (type: string) => void
}

function makeFakeVisualViewport(height: number, offsetTop = 0): FakeVisualViewport {
    const listeners = new Map<string, Set<() => void>>()
    return {
        height,
        offsetTop,
        scale: 1,
        addEventListener(type, listener) {
            const set = listeners.get(type) ?? new Set()
            set.add(listener)
            listeners.set(type, set)
        },
        removeEventListener(type, listener) {
            listeners.get(type)?.delete(listener)
        },
        dispatch(type) {
            for (const listener of listeners.get(type) ?? []) listener()
        },
    }
}

let frames: FrameRequestCallback[]
function flushFrames() {
    const pending = [...frames]
    frames.length = 0
    for (const callback of pending) callback(0)
}

beforeEach(() => {
    resetViewportStore()
    vi.stubGlobal('innerWidth', 375)
    vi.stubGlobal('innerHeight', 700)
    vi.stubGlobal('matchMedia', () => ({
        matches: true,
        addEventListener: () => {},
        removeEventListener: () => {},
    }))
    frames = []
    vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => {
        frames.push(callback)
        return frames.length
    })
    vi.stubGlobal('cancelAnimationFrame', (id: number) => {
        frames[id - 1] = () => {}
    })
})

afterEach(() => {
    cleanup()
    resetViewportStore()
    document.documentElement.style.removeProperty('--map-viewport-height')
    document.documentElement.style.removeProperty('--map-viewport-offset-top')
    vi.unstubAllGlobals()
    vi.restoreAllMocks()
})

function renderShell() {
    return render(
        createElement(MemoryRouter, { initialEntries: ['/?lang=en'] }, createElement(MapShell)),
    )
}

it('publishes the shared snapshot to both CSS variables and the sheet height', () => {
    const viewport = makeFakeVisualViewport(600)
    vi.stubGlobal('visualViewport', viewport)
    renderShell()
    const root = document.getElementById('map-shell')!
    const style = document.documentElement.style
    // Same snapshot -> the shell fills 600px of visible height and the mobile
    // half menu is min(nearbyHeight 326, 600 - 46) = 326px.
    expect(style.getPropertyValue('--map-viewport-height')).toBe('600px')
    expect(style.getPropertyValue('--map-viewport-offset-top')).toBe('0px')
    expect(root.style.getPropertyValue('--sheet-height')).toBe('326px')
})

it('re-syncs the CSS variables and sheet height after a visualViewport resize', () => {
    const viewport = makeFakeVisualViewport(600)
    vi.stubGlobal('visualViewport', viewport)
    renderShell()
    const root = document.getElementById('map-shell')!
    act(() => {
        viewport.height = 500
        viewport.offsetTop = 80
        viewport.dispatch('resize')
    })
    expect(frames.length).toBeGreaterThan(0)
    act(() => flushFrames())
    const style = document.documentElement.style
    // Height and the keyboard pan come from the same snapshot: the shell grows
    // by offsetTop, while the sheet is capped against the visible height alone.
    expect(style.getPropertyValue('--map-viewport-height')).toBe('500px')
    expect(style.getPropertyValue('--map-viewport-offset-top')).toBe('80px')
    expect(root.style.getPropertyValue('--sheet-height')).toBe('326px')
})

it('anchors the phone contribution sheet to the shared snapshot without subtracting the keyboard pan twice', async () => {
    const viewport = makeFakeVisualViewport(500, 120)
    vi.stubGlobal('visualViewport', viewport)
    const { container } = render(
        createElement(MemoryRouter, { initialEntries: ['/?lang=en'] }, createElement(MapShell)),
    )
    fireEvent.click(screen.getByRole('button', { name: 'Contribute' }))
    fireEvent.click(container.querySelector('.product-map')!, { clientX: 200, clientY: 200 })
    await screen.findByRole('form', { name: 'Contribution draft' })
    const sheet = container.querySelector<HTMLElement>('.mobile-sheet.mobile-contribution')!
    // One source of truth: the shell grows by the pan and exposes it once.
    expect(document.documentElement.style.getPropertyValue('--map-viewport-height')).toBe('500px')
    expect(document.documentElement.style.getPropertyValue('--map-viewport-offset-top')).toBe(
        '120px',
    )
    // The sheet must not keep its own keyboard/viewport variables; that would
    // subtract the pan a second time.
    expect(sheet.style.getPropertyValue('--contribution-keyboard-offset')).toBe('')
    expect(sheet.style.getPropertyValue('--contribution-viewport-height')).toBe('')
    // The sheet height is derived from the visible height only (500 - 46).
    const root = document.getElementById('map-shell')!
    expect(root.style.getPropertyValue('--sheet-height')).toBe('454px')
    await waitFor(() =>
        expect(container.querySelector('.mobile-sheet.mobile-contribution')).not.toBeNull(),
    )
})
